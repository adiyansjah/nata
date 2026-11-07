//! OS admission values, queried before creating any spill file. Unsafe is
//! confined to the existing libc statvfs ABI; all operator/codec code is safe.
use std::{io, path::Path};

#[cfg(unix)]
#[allow(
    unsafe_code,
    reason = "statvfs has no safe standard-library interface; checked CString and success-only initialization bound this ABI call"
)]
pub fn available_bytes(base: &Path) -> io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(base.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid spill directory"))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: path is NUL-terminated, stat points to a properly aligned writable
    // object of the ABI's exact type, and neither pointer escapes the call.
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: storage was zero-initialized, and successful statvfs fills its
    // documented fields; reserved fields also remain initialized.
    let stat = unsafe { stat.assume_init() };
    u64::try_from(u128::from(stat.f_bavail) * u128::from(stat.f_frsize))
        .map_err(|_| io::Error::other("spill volume size overflow"))
}
#[cfg(not(unix))]
pub fn available_bytes(_base: &Path) -> io::Result<u64> {
    Err(io::Error::other(
        "spill volume admission unavailable on this platform",
    ))
}
pub fn descriptor_allowance(workers: usize) -> io::Result<usize> {
    let limits = std::fs::read_to_string("/proc/self/limits")?;
    let soft = limits
        .lines()
        .find_map(|line| {
            line.strip_prefix("Max open files")
                .and_then(|s| s.split_whitespace().next())
        })
        .ok_or_else(|| io::Error::other("spill descriptor admission unavailable"))?;
    let soft = if soft == "unlimited" {
        usize::MAX
    } else {
        soft.parse()
            .map_err(|_| io::Error::other("invalid descriptor allowance"))?
    };
    let open = std::fs::read_dir("/proc/self/fd")?.count();
    Ok(soft
        .saturating_sub(open.saturating_add(workers).saturating_add(8))
        .min(32))
}
