//! Explicit restart cleanup. Locks prove ownership even across PID reuse.
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

pub(super) fn lock_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let f = options.open(path)?;
    if !f.metadata()?.is_file() {
        return Err(io::Error::other("invalid spill lock"));
    }
    Ok(f)
}

/// Remove only unlocked, registry-created query directories. Live queries are
/// skipped. Unknown files, links and directories are never followed or adopted.
/// Run against a trusted, process-owned spill base, also used by Registry::new.
pub fn cleanup_stale(base: &Path) -> io::Result<usize> {
    if !fs::symlink_metadata(base)?.is_dir() {
        return Err(io::Error::other("invalid spill base"));
    }
    let guard = File::open(base)?;
    guard.lock()?;
    let mut removed = 0;
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(suffix) = name.strip_prefix("nata-spill-") else {
            continue;
        };
        if !suffix
            .split('-')
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            || !entry.file_type()?.is_dir()
        {
            continue;
        }
        let dir = entry.path();
        let owner = dir.join("owner.lock");
        if !fs::symlink_metadata(&owner).is_ok_and(|m| m.is_file()) {
            continue;
        }
        let lock = lock_file(&owner)?;
        if lock.try_lock().is_err() {
            continue;
        }
        let files = fs::read_dir(&dir)?.collect::<io::Result<Vec<_>>>()?;
        if files.iter().any(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !e.file_type().is_ok_and(|t| t.is_file())
                || !(name == "owner.lock"
                    || name
                        .strip_prefix("partition-")
                        .and_then(|s| s.strip_suffix(".ipc"))
                        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())))
        }) {
            continue;
        }
        for file in files {
            fs::remove_file(file.path())?;
        }
        fs::remove_dir(dir)?;
        removed += 1;
    }
    Ok(removed)
}
