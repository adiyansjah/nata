//! Bounded native trials; scale the corruption campaign via NATA_SPILL_CASES.
use arrow_array::{ArrayRef, BinaryArray, RecordBatch, UInt64Array};
use nata_exec::spill::{MAX_ROWS, Registry, Workspace, schema};
use nata_sched::memory::MemoryManager;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Base(PathBuf);
impl Base {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "nata-spill-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("test directory");
        Self(path)
    }
    fn empty(&self) {
        assert_eq!(fs::read_dir(&self.0).expect("base").count(), 0);
    }
}
impl Drop for Base {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("test cleanup");
    }
}
fn batch(n: usize) -> RecordBatch {
    let hashes: ArrayRef = Arc::new(UInt64Array::from_iter_values((0..n).map(|n| n as u64)));
    let keys: ArrayRef = Arc::new(BinaryArray::from_iter_values((0..n).map(|_| b"key\0\xff")));
    let states: ArrayRef = Arc::new(BinaryArray::from_iter_values((0..n).map(|_| [0u8; 40])));
    RecordBatch::try_new(Arc::new(schema()), vec![hashes, keys, states]).expect("trusted batch")
}
fn pick(random: u64, bound: usize) -> usize {
    usize::try_from(random % u64::try_from(bound).expect("bound")).expect("bounded index")
}
fn memory() -> Arc<MemoryManager> {
    MemoryManager::new(4 << 20, 4096, 1).expect("memory")
}
fn clean(m: &MemoryManager) {
    let s = m.snapshot();
    assert_eq!((s.held, s.live_grants), (0, 0));
    assert!(s.peak <= s.budget);
}
#[test]
fn frames_round_trip_empty_full_and_multiple() {
    let base = Base::new();
    let m = memory();
    {
        let registry = Registry::new(&base.0, 4 << 20, 32, Arc::clone(&m)).expect("registry");
        let file = registry.create().expect("file");
        let mut h = file.open().expect("handle");
        let mut w = Workspace::new(&m).expect("workspace");
        for n in [0, 1, MAX_ROWS] {
            w.write(&mut h, &batch(n), 15, 8).expect("write");
        }
        h.rewind().expect("rewind");
        for n in [0, 1, MAX_ROWS] {
            assert_eq!(
                w.read(&mut h, 15, 8).expect("read").expect("frame").batch(),
                &batch(n)
            );
        }
        assert!(w.read(&mut h, 15, 8).expect("EOF").is_none());
        let s = registry.snapshot();
        assert!(s.bytes_written > 0);
        assert_eq!(s.bytes_read, s.bytes_written);
        assert_eq!(s.live_bytes, s.bytes_written);
        drop(h);
        drop(file);
        assert_eq!(
            (
                registry.snapshot().live_bytes,
                registry.snapshot().files,
                registry.snapshot().open
            ),
            (0, 0, 0)
        );
    }
    base.empty();
    clean(&m);
}
#[test]
fn descriptors_are_capped_and_release_on_drop() {
    let base = Base::new();
    let m = memory();
    for allowance in [3, 32, 64] {
        let registry = Registry::new(&base.0, 4096, allowance, Arc::clone(&m)).expect("registry");
        let f = registry.create().expect("file");
        let cap = allowance.min(32);
        let handles: Vec<_> = (0..cap).map(|_| f.open().expect("handle")).collect();
        assert!(f.open().is_err(), "descriptor cap removed");
        assert!(registry.create().is_err(), "creation bypasses cap");
        assert_eq!(registry.snapshot().peak_open, cap);
        drop(handles);
        drop(f.open().expect("released slot"));
        drop(f);
        drop(registry);
        base.empty();
    }
    clean(&m);
}
#[test]
fn quota_short_write_error_cancel_and_panic_leave_no_files() {
    let base = Base::new();
    let m = memory();
    for reason in 0..5 {
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let registry = Registry::new(
                &base.0,
                if reason == 0 { 8 } else { 1 << 20 },
                32,
                Arc::clone(&m),
            )
            .expect("registry");
            let f = registry.create().expect("file");
            let mut h = f.open().expect("handle");
            let mut w = Workspace::new(&m).expect("workspace");
            if reason == 1 {
                registry.inject_write_failure(17);
            }
            let r = w.write(&mut h, &batch(8), 0, 0);
            if reason <= 1 {
                assert!(
                    r.expect_err("injected error")
                        .to_string()
                        .contains(f.path().to_str().expect("path"))
                );
            } else {
                r.expect("write");
            }
            if reason == 3 {
                panic!("task panic");
            }
            if reason == 4 {
                m.cancel();
            }
        }));
        assert_eq!(run.is_err(), reason == 3);
        base.empty();
        clean(&m);
    }
}
#[test]
fn private_exclusive_files_and_no_symlink_adoption() {
    let base = Base::new();
    let m = memory();
    let registry = Registry::new(&base.0, 4096, 32, Arc::clone(&m)).expect("registry");
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        assert_eq!(
            fs::metadata(registry.directory())
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let bait = registry.directory().join("partition-0.ipc");
        let target = base.0.join("untouched");
        fs::write(&target, b"untouched").expect("bait");
        symlink(&target, &bait).expect("link");
        assert!(registry.create().is_err());
        assert_eq!(fs::read(&target).expect("target"), b"untouched");
        fs::remove_file(&bait).expect("remove bait");
        fs::remove_file(target).expect("remove target");
    }
    let file = registry.create().expect("exclusive file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(file.path())
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    drop(file);
    drop(registry);
    base.empty();
    clean(&m);
}
#[test]
fn seeded_corruption_rejected_before_arrow_allocation() {
    let base = Base::new();
    let m = memory();
    let seed = std::env::var("NATA_SPILL_SEED")
        .ok()
        .map_or(0x0005_2026_1107, |n| n.parse::<u64>().expect("seed"));
    let cases = std::env::var("NATA_SPILL_CASES")
        .ok()
        .map_or(64, |n| n.parse::<usize>().expect("cases"));
    println!("spill corruption seed={seed} cases={cases} per shape; max payload=65536");
    let mut random = seed.max(1);
    for _ in 0..cases {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        let registry = Registry::new(&base.0, 1 << 20, 32, Arc::clone(&m)).expect("registry");
        let f = registry.create().expect("file");
        let mut h = f.open().expect("handle");
        let mut workspace = Workspace::new(&m).expect("workspace");
        workspace
            .write(&mut h, &batch(1 + pick(random, MAX_ROWS)), 7, 2)
            .expect("write");
        let original = fs::read(f.path()).expect("original");
        for shape in 0..4 {
            let mut broken = original.clone();
            match shape {
                0 => broken.truncate(1 + pick(random, original.len() - 1)),
                1 => {
                    let i = pick(random, broken.len());
                    broken[i] ^= 1;
                }
                2 => broken[20..28].copy_from_slice(&(u64::MAX - 7).to_le_bytes()),
                _ => broken[20..28].copy_from_slice(&(original.len() as u64 + 1).to_le_bytes()),
            }
            fs::write(f.path(), &broken).expect("corrupt");
            h.rewind().expect("rewind");
            let e = workspace
                .read(&mut h, 7, 2)
                .expect_err("corruption accepted");
            assert!(e.to_string().contains(f.path().to_str().expect("path")));
        }
    }
    base.empty();
    clean(&m);
}
#[test]
fn altered_inner_offsets_rejected_even_with_repaired_checksum() {
    let base = Base::new();
    let m = memory();
    {
        let registry = Registry::new(&base.0, 1 << 20, 32, Arc::clone(&m)).expect("registry");
        let f = registry.create().expect("file");
        let mut h = f.open().expect("handle");
        let mut w = Workspace::new(&m).expect("workspace");
        w.write(&mut h, &batch(4), 0, 0).expect("write");
        let mut bytes = Vec::new();
        h.rewind().expect("rewind");
        h.read_to_end(&mut bytes).expect("test frame");
        let schema_len = u32::from_le_bytes(bytes[36..40].try_into().expect("length")) as usize;
        let start = 40 + schema_len;
        let meta_len =
            u32::from_le_bytes(bytes[start + 4..start + 8].try_into().expect("length")) as usize;
        let message =
            arrow_ipc::root_as_message(&bytes[start + 8..start + 8 + meta_len]).expect("metadata");
        let offset = message
            .header_as_record_batch()
            .expect("batch")
            .buffers()
            .expect("buffers")
            .get(3)
            .offset();
        let offset = usize::try_from(offset).expect("offset");
        let index = start + 8 + meta_len + offset;
        bytes[index..index + 4].copy_from_slice(&i32::MAX.to_le_bytes());
        let mut crc = crc32fast::Hasher::new();
        crc.update(&bytes[..28]);
        crc.update(&bytes[32..]);
        bytes[28..32].copy_from_slice(&crc.finalize().to_le_bytes());
        fs::write(f.path(), bytes).expect("corrupt");
        h.seek(SeekFrom::Start(0)).expect("rewind");
        assert!(w.read(&mut h, 0, 0).is_err(), "inner layout unchecked");
    }
    base.empty();
    clean(&m);
}

struct Ready(std::sync::mpsc::SyncSender<()>);
impl nata_sched::memory::Wake for Ready {
    fn wake(&self) {
        let _ = self.0.try_send(());
    }
}
fn wait<T>(ticket: &nata_exec::spill::Ticket<T>) -> std::io::Result<T> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    if let Some(result) = ticket.poll(Ready(tx)) {
        return result;
    }
    rx.recv_timeout(std::time::Duration::from_secs(10))
        .expect("disk wake deadline");
    ticket
        .poll(Ready(std::sync::mpsc::sync_channel(1).0))
        .expect("completed")
}
#[test]
fn disk_service_wakes_before_or_after_park_and_survives_task_panic() {
    use nata_exec::spill::{IoService, Submit};
    let base = Base::new();
    let m = memory();
    {
        let registry = Registry::new(&base.0, 1 << 20, 32, Arc::clone(&m)).expect("registry");
        let service = IoService::new(2, &m).expect("disk service");
        for panic in [false, true, false] {
            let file = registry.create().expect("file");
            let mut workspace = Workspace::new(&m).expect("workspace");
            let job = Box::new(move || {
                let mut handle = file.open()?;
                workspace.write(&mut handle, &batch(8), 0, 0)?;
                if panic {
                    panic!("disk task panic");
                }
                Ok(42)
            });
            let Submit::Queued(ticket) = service.submit(job) else {
                panic!("queue");
            };
            let r = wait(&ticket);
            assert_eq!(r.is_err(), panic);
            assert_eq!(
                (
                    registry.snapshot().files,
                    registry.snapshot().open,
                    registry.snapshot().live_bytes
                ),
                (0, 0, 0)
            );
        }
    }
    base.empty();
    clean(&m);
}
#[test]
fn disk_queue_is_bounded_and_cancel_drops_queued_job_before_wake() {
    use nata_exec::spill::{IoService, Submit};
    let base = Base::new();
    let m = memory();
    {
        let registry = Registry::new(&base.0, 1 << 20, 32, Arc::clone(&m)).expect("registry");
        let service = IoService::new(1, &m).expect("disk service");
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(1);
        let Submit::Queued(first) = service.submit(Box::new(move || {
            started_tx.send(()).expect("started");
            resume_rx.recv().expect("resume");
            Ok(1)
        })) else {
            panic!("queue");
        };
        started_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("running");
        let file = registry.create().expect("file");
        let Submit::Queued(second) = service.submit(Box::new(move || {
            drop(file);
            panic!("cancelled job ran");
        })) else {
            panic!("queue");
        };
        assert!(
            matches!(service.submit(Box::new(|| Ok(3))), Submit::Full(_)),
            "unbounded I/O queue"
        );
        service.cancel();
        resume_tx.send(()).expect("resume");
        assert_eq!(wait(&first).expect("in flight finishes"), 1);
        assert_eq!(
            wait(&second).expect_err("cancelled").kind(),
            std::io::ErrorKind::Interrupted
        );
        assert_eq!(
            (registry.snapshot().files, registry.snapshot().open),
            (0, 0)
        );
    }
    base.empty();
    clean(&m);
}

#[test]
fn decoded_buffers_retain_reservation_and_cannot_multiply_workspace() {
    let base = Base::new();
    let m = memory();
    {
        let registry = Registry::new(&base.0, 1 << 20, 32, Arc::clone(&m)).expect("registry");
        let file = registry.create().expect("file");
        let mut handle = file.open().expect("handle");
        let mut workspace = Workspace::new(&m).expect("workspace");
        workspace
            .write(&mut handle, &batch(16), 0, 0)
            .expect("write");
        handle.rewind().expect("rewind");
        let frame = workspace
            .read(&mut handle, 0, 0)
            .expect("read")
            .expect("frame");
        let held = m.snapshot().held;
        assert!(
            workspace.read(&mut handle, 0, 0).is_err(),
            "workspace multiplied while borrowed"
        );
        drop(workspace);
        assert_eq!(
            m.snapshot().held,
            held,
            "decoded buffers lost their reservation"
        );
        assert_eq!(frame.batch().num_rows(), 16);
        drop(frame);
        assert!(m.snapshot().held < held);
    }
    base.empty();
    clean(&m);
}
