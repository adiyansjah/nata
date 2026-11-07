use super::invalid;
use nata_sched::memory::{Grant, MemoryManager, Reservation};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

const FD_CAP: usize = 32;
const FILE_CHARGE: usize = 512;
static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default, Clone, Copy)]
pub struct SpillStats {
    pub bytes_written: u64,
    pub bytes_read: u64,
    pub live_bytes: u64,
    pub peak_bytes: u64,
    pub files: usize,
    pub open: usize,
    pub peak_open: usize,
}
struct Ledger {
    stats: SpillStats,
    next: u64,
    // Native fault hook, query-local; never controlled by spill file contents.
    fail_after: Option<u64>,
    fault_reserved: u64,
}
/// One private directory per query. No caller-supplied file names are accepted.
/// D and the process descriptor allowance are supplied by admission.
pub struct Registry {
    directory: PathBuf,
    quota: u64,
    descriptor_cap: usize,
    state: Mutex<Ledger>,
    memory: Arc<MemoryManager>,
    _grant: Grant,
}
fn reserve(memory: &Arc<MemoryManager>, bytes: usize) -> io::Result<Grant> {
    match MemoryManager::request(memory, 0, bytes) {
        Ok(Reservation::Granted(g)) => Ok(g),
        _ => Err(io::Error::other("spill registry memory limit")),
    }
}
impl Registry {
    /// `base` is an admission-selected directory on disk, not a query value.
    /// Its final component must already be a real directory, never a symlink.
    pub fn new(
        base: &Path,
        quota: u64,
        process_allowance: usize,
        memory: Arc<MemoryManager>,
    ) -> io::Result<Arc<Self>> {
        let bytes = base
            .as_os_str()
            .len()
            .checked_mul(2)
            .and_then(|n| n.checked_add(4096))
            .ok_or_else(invalid)?;
        let grant = reserve(&memory, bytes)?;
        if quota == 0 || process_allowance == 0 || !fs::symlink_metadata(base)?.is_dir() {
            return Err(io::Error::other("invalid spill admission"));
        }
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        // Exclusive creation prevents collisions, including pre-existing links.
        // Never adopt or delete a directory we did not create ourselves.
        for _ in 0..128 {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let directory = base.join(format!("nata-spill-{}-{id}", std::process::id()));
            match builder.create(&directory) {
                Ok(()) => {
                    return Ok(Arc::new(Self {
                        directory,
                        quota,
                        descriptor_cap: FD_CAP.min(process_allowance),
                        state: Mutex::new(Ledger {
                            stats: SpillStats::default(),
                            next: 0,
                            fail_after: None,
                            fault_reserved: 0,
                        }),
                        memory,
                        _grant: grant,
                    }));
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("spill directory collision limit"))
    }
    fn lock(&self) -> MutexGuard<'_, Ledger> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn snapshot(&self) -> SpillStats {
        self.lock().stats
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    /// Inject a short write followed by ENOSPC after this many query bytes.
    pub fn inject_write_failure(&self, after: u64) {
        self.lock().fail_after = Some(after);
    }
    pub fn create(self: &Arc<Self>) -> io::Result<Arc<SpillFile>> {
        let bytes = FILE_CHARGE
            .checked_add(self.directory.as_os_str().len())
            .ok_or_else(invalid)?;
        let grant = reserve(&self.memory, bytes)?;
        let id = {
            let mut s = self.lock();
            let id = s.next;
            s.next = id.checked_add(1).ok_or_else(invalid)?;
            id
        };
        let path = self.directory.join(format!("partition-{id}.ipc"));
        let slot = self.acquire()?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        // Only count files whose exclusive creation actually succeeded.
        self.lock().stats.files += 1;
        let spill = Arc::new(SpillFile {
            path,
            registry: Arc::clone(self),
            bytes: Mutex::new(0),
            _grant: grant,
        });
        drop(file);
        drop(slot);
        Ok(spill)
    }
    fn acquire(self: &Arc<Self>) -> io::Result<Descriptor> {
        let mut s = self.lock();
        if s.stats.open >= self.descriptor_cap {
            return Err(io::Error::other("spill descriptor limit"));
        }
        s.stats.open += 1;
        s.stats.peak_open = s.stats.peak_open.max(s.stats.open);
        Ok(Descriptor(Arc::clone(self)))
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        // File and handle owners retain the registry until their own cleanup.
        // Remove only the exclusively-created, query-private directory.
        let _ = fs::remove_dir(&self.directory);
    }
}
struct Descriptor(Arc<Registry>);
impl Drop for Descriptor {
    fn drop(&mut self) {
        self.0.lock().stats.open -= 1;
    }
}
/// Removes its exclusively-created file on the last owner, including unwind.
/// A handle retains this owner; it cannot outlive cleanup or bypass the cap.
pub struct SpillFile {
    path: PathBuf,
    registry: Arc<Registry>,
    bytes: Mutex<u64>,
    _grant: Grant,
}
impl SpillFile {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn open(self: &Arc<Self>) -> io::Result<SpillHandle> {
        let slot = self.registry.acquire()?;
        // Within a private directory, and only our exclusive-created files.
        // Reject a replacement link before opening as a defence in depth.
        let metadata = fs::symlink_metadata(&self.path)?;
        if !metadata.is_file() {
            return Err(invalid());
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&self.path)?;
        Ok(SpillHandle {
            file,
            owner: Arc::clone(self),
            _slot: slot,
        })
    }
}
impl Drop for SpillFile {
    fn drop(&mut self) {
        if fs::remove_file(&self.path).is_ok() {
            let mut s = self.registry.lock();
            let bytes = *self
                .bytes
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            s.stats.live_bytes -= bytes;
            s.stats.files -= 1;
        }
    }
}
pub struct SpillHandle {
    // File closes before the owner removes it and releases the descriptor slot.
    file: File,
    owner: Arc<SpillFile>,
    _slot: Descriptor,
}
impl SpillHandle {
    pub fn size(&self) -> io::Result<u64> {
        let size = self.file.metadata()?.len();
        let expected = *self
            .owner
            .bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if size != expected {
            return Err(invalid());
        }
        Ok(size)
    }
    pub fn path(&self) -> &Path {
        self.owner.path()
    }
    /// Reserve the whole append against D before touching the filesystem.
    pub(crate) fn append(&mut self, frame: &[u8]) -> io::Result<()> {
        let registry = &self.owner.registry;
        let amount = u64::try_from(frame.len()).map_err(|_| invalid())?;
        let mut own = self
            .owner
            .bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut s = registry.lock();
        let next = s.stats.live_bytes.checked_add(amount).ok_or_else(invalid)?;
        if next > registry.quota {
            return Err(io::Error::other("spill disk quota exceeded"));
        }
        s.stats.live_bytes = next;
        s.stats.peak_bytes = s.stats.peak_bytes.max(next);
        *own += amount;
        let allowed = s.fail_after.map_or(amount, |limit| {
            limit.saturating_sub(s.fault_reserved).min(amount)
        });
        // Serialize append fault accounting, but never hold the registry lock
        // across a filesystem operation or a memory-service callback.
        s.fault_reserved += allowed;
        drop(s);
        self.file.seek(SeekFrom::End(0))?;
        let allowed_bytes = usize::try_from(allowed).map_err(|_| invalid())?;
        let mut written = 0;
        while written < allowed_bytes {
            let n = match self.file.write(&frame[written..allowed_bytes]) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                other => other?,
            };
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "short spill write",
                ));
            }
            registry.lock().stats.bytes_written += n as u64;
            written += n;
        }
        if allowed != amount {
            return Err(io::Error::from_raw_os_error(28));
        }
        Ok(())
    }
}
impl Read for SpillHandle {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read(buf)?;
        self.owner.registry.lock().stats.bytes_read += n as u64;
        Ok(n)
    }
}
impl Seek for SpillHandle {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}
