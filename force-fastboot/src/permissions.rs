//!
//! Permission-checking helpers for serial port access.
//!
//! Determines whether an error is permission-related and detects root execution.

use std::io;

/// Returns `true` if `error` is (or wraps) a permission-denied error, either via
/// [`std::io::ErrorKind::PermissionDenied`], a POSIX `EACCES`/`EPERM` OS error code,
/// or a message containing "permission denied" or "access is denied".
pub fn is_permission_error(error: &anyhow::Error) -> bool {
    if let Some(io_err) = error.downcast_ref::<io::Error>() {
        if io_err.kind() == io::ErrorKind::PermissionDenied {
            return true;
        }

        #[cfg(unix)]
        {
            if let Some(os_err) = io_err.raw_os_error() {
                if os_err == libc::EACCES || os_err == libc::EPERM {
                    return true;
                }
            }
        }
    }

    let msg = format!("{error:#}").to_lowercase();
    msg.contains("permission denied")
        || msg.contains("access is denied")
        || msg.contains("access denied")
}

/// Returns `true` if the process is running as root (uid 0) on Unix.
/// Always returns `false` on non-Unix platforms.
#[cfg_attr(unix, expect(unsafe_code))]
pub fn is_running_as_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid()` takes no arguments, returns a valid uid_t, and is safe to call from
        // any context per POSIX. It cannot fail or cause undefined behavior.
        unsafe { libc::geteuid() == 0 }
    }

    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn permission_denied_via_errorkind() {
        let err = anyhow::Error::new(io::Error::new(io::ErrorKind::PermissionDenied, "nope"));
        assert!(is_permission_error(&err));
    }

    #[test]
    fn permission_denied_via_message() {
        let err = anyhow::anyhow!("could not open port: Permission denied");
        assert!(is_permission_error(&err));
    }

    #[test]
    fn permission_denied_via_access_is_denied() {
        let err = anyhow::anyhow!("Access is denied");
        assert!(is_permission_error(&err));
    }

    #[test]
    fn not_permission_error() {
        let err = anyhow::anyhow!("device busy");
        assert!(!is_permission_error(&err));
    }

    #[test]
    fn root_detection_returns_bool() {
        let _result = is_running_as_root();
    }
}
