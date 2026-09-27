//! `fs.access` with `R_OK` / `R_OK | W_OK`.

use std::path::Path;

/// `fs.access(path, R_OK)`, or `R_OK | W_OK` with `write`.
#[cfg(unix)]
pub(crate) fn access(path: &Path, write: bool) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let mode = if write {
        libc::R_OK | libc::W_OK
    } else {
        libc::R_OK
    };
    // SAFETY: `c_path` is a valid NUL-terminated string for the duration of the call.
    if unsafe { libc::access(c_path.as_ptr(), mode) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// `fs.access`: existence, plus the read-only flag for `write`.
#[cfg(not(unix))]
pub(crate) fn access(path: &Path, write: bool) -> std::io::Result<()> {
    let metadata = std::fs::metadata(path)?;
    if write && metadata.permissions().readonly() {
        return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    }
    Ok(())
}
