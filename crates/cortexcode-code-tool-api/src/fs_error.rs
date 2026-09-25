//! Node-style filesystem error messages.
//!
//! Tool errors reach the model as the thrown error's `message`, so an fs
//! failure has to read the way Node (libuv) words it, e.g.
//! `ENOENT: no such file or directory, access '/x'`.

use std::fmt;

/// An fs error rendered as `<CODE>: <description>, <syscall>[ '<path>']`.
#[derive(Debug)]
pub struct NodeFsError {
    pub code: String,
    pub message: String,
    pub source: std::io::Error,
}

impl fmt::Display for NodeFsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for NodeFsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// libuv's code and description for an io error (the common fs ones).
fn uv_code(err: &std::io::Error) -> Option<(&'static str, &'static str)> {
    use std::io::ErrorKind as K;
    Some(match err.kind() {
        K::NotFound => ("ENOENT", "no such file or directory"),
        K::PermissionDenied => ("EACCES", "permission denied"),
        K::IsADirectory => ("EISDIR", "illegal operation on a directory"),
        K::NotADirectory => ("ENOTDIR", "not a directory"),
        K::AlreadyExists => ("EEXIST", "file already exists"),
        K::DirectoryNotEmpty => ("ENOTEMPTY", "directory not empty"),
        K::ReadOnlyFilesystem => ("EROFS", "read-only file system"),
        K::StorageFull => ("ENOSPC", "no space left on device"),
        K::FileTooLarge => ("EFBIG", "file too large"),
        K::ResourceBusy => ("EBUSY", "resource busy or locked"),
        K::ExecutableFileBusy => ("ETXTBSY", "text file is busy"),
        K::CrossesDevices => ("EXDEV", "cross-device link not permitted"),
        K::InvalidFilename => ("ENAMETOOLONG", "name too long"),
        K::InvalidInput => ("EINVAL", "invalid argument"),
        K::OutOfMemory => ("ENOMEM", "not enough memory"),
        _ => return None,
    })
}

/// Wrap an io error the way Node reports it from `syscall` on `path`.
/// `path` is `None` for calls on an open handle (e.g. `read`).
pub fn node_fs_error(err: std::io::Error, syscall: &str, path: Option<&str>) -> NodeFsError {
    let (code, description) = match uv_code(&err) {
        Some((code, description)) => (code.to_string(), description.to_string()),
        None => ("UNKNOWN".to_string(), err.to_string()),
    };
    let message = match path {
        Some(path) => format!("{code}: {description}, {syscall} '{path}'"),
        None => format!("{code}: {description}, {syscall}"),
    };
    NodeFsError {
        code,
        message,
        source: err,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_node() {
        let e = node_fs_error(
            std::io::Error::from(std::io::ErrorKind::NotFound),
            "access",
            Some("/tmp/x"),
        );
        assert_eq!(
            e.to_string(),
            "ENOENT: no such file or directory, access '/tmp/x'"
        );
        assert_eq!(e.code, "ENOENT");
        let e = node_fs_error(
            std::io::Error::from(std::io::ErrorKind::IsADirectory),
            "read",
            None,
        );
        assert_eq!(
            e.to_string(),
            "EISDIR: illegal operation on a directory, read"
        );
    }

    #[test]
    fn real_missing_file() {
        let err = std::fs::metadata("/definitely/not/here").unwrap_err();
        let e = node_fs_error(err, "open", Some("/definitely/not/here"));
        assert!(e
            .to_string()
            .starts_with("ENOENT: no such file or directory, open"));
    }
}
