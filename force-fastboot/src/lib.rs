#![deny(unsafe_code, missing_docs)]

pub mod permissions;
pub mod protocol;
pub mod serial;
pub mod spinner;
pub mod udev;

use std::io;

use terminal_output::chrome::{banner, notice_box, status_line, Tone};
use thiserror::Error;

use serial::{PortDiscovery, SystemPortDiscovery};

#[derive(Debug, Error)]
pub enum ForceFastbootError {
    #[error("no MTK preloader device found: {0}")]
    NoDevice(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("serial port error: {0}")]
    Serial(String),
    #[error("protocol handshake failed: {0}")]
    Protocol(String),
    #[error("udev setup failed: {0}")]
    Udev(String),
}

#[derive(Debug, Clone, Default)]
pub struct ForceFastbootOptions {
    pub port: Option<String>,
    pub no_auto_udev: bool,
}

pub fn run_force_fastboot(options: &ForceFastbootOptions) -> Result<(), ForceFastbootError> {
    run_force_fastboot_with_discovery(options, &SystemPortDiscovery)
}

pub fn run_force_fastboot_with_discovery(
    options: &ForceFastbootOptions,
    discovery: &dyn PortDiscovery,
) -> Result<(), ForceFastbootError> {
    if permissions::is_running_as_root() && !cfg!(windows) {
        eprintln!(
            "{}",
            notice_box(
                Tone::Warning,
                "root execution",
                "Running the whole script as root is unnecessary. Prefer a normal user when possible."
            )
        );
    }

    println!("{}", banner("FORCE FASTBOOT"));
    let auto_udev = !options.no_auto_udev;
    let candidate = if let Some(port) = &options.port {
        serial::candidate_for_device(port, discovery)
    } else {
        serial::wait_for_port(discovery, auto_udev)
            .map_err(|e| ForceFastbootError::NoDevice(format!("{e:#}")))?
    };

    println!(
        "{}",
        status_line(Tone::Info, "port", &format!("opening {}", candidate.device))
    );
    let mut port = serial::open_with_permission_recovery(&candidate, discovery, auto_udev)
        .map_err(|e| {
        if permissions::is_permission_error(&e) {
            ForceFastbootError::PermissionDenied(format!("{e:#}"))
        } else {
            ForceFastbootError::Serial(format!("{e:#}"))
        }
    })?;

    {
        let _spinner = spinner::StatusSpinner::new("Waiting for preloader handshake byte...");
        protocol::force_fastboot(port.as_mut()).map_err(|e| match e.kind() {
            io::ErrorKind::TimedOut => ForceFastbootError::Protocol(format!("timed out: {e}")),
            _ => ForceFastbootError::Protocol(format!("{e}")),
        })?;
    }

    println!(
        "{}",
        status_line(Tone::Success, "handshake", "FASTBOOT command sent")
    );
    Ok(())
}
