use std::io;

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

#[allow(unsafe_code)]
pub fn is_running_as_root() -> bool {
    #[cfg(unix)]
    {
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
