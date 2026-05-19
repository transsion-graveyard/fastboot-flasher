#![deny(unsafe_code, missing_docs)]
//!
//! Force an MTK preloader device into fastboot mode by detecting the serial port,
//! handling permissions (including automatic udev rule installation on Linux),
//! and sending the fastboot handshake protocol.
//!
//! The main entry points are [`run_force_fastboot`] and [`run_force_fastboot_with_discovery`].

pub mod permissions;
pub mod protocol;
pub mod serial;
pub mod spinner;
pub mod udev;

use std::io;

use terminal_output::chrome::{simple_banner, simple_notice_box, simple_status_line, Tone};
use thiserror::Error;

use serial::{PortDiscovery, SystemPortDiscovery};

/// Errors that can occur during force-fastboot operations.
#[derive(Debug, Error)]
pub enum ForceFastbootError {
    /// No MTK preloader device could be found on any serial port.
    #[error("no MTK preloader device found: {0}")]
    NoDevice(String),
    /// Insufficient permissions to open the serial port, and automatic recovery failed.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// Opening or communicating over the serial port failed.
    #[error("serial port error: {0}")]
    Serial(String),
    /// The preloader handshake protocol failed (e.g. timeout waiting for the start byte).
    #[error("protocol handshake failed: {0}")]
    Protocol(String),
    /// Automatic udev rule installation failed.
    #[error("udev setup failed: {0}")]
    Udev(String),
}

/// Configuration options for [`run_force_fastboot`].
#[derive(Debug, Clone, Default)]
pub struct ForceFastbootOptions {
    /// An explicit serial port path to use (e.g. `/dev/ttyUSB0`).
    /// When `None`, the device is auto-detected.
    pub port: Option<String>,
    /// If `true`, skip automatic udev rule installation on Linux when a permission
    /// error is encountered opening the serial port.
    pub no_auto_udev: bool,
}

/// Convenience wrapper around [`run_force_fastboot_with_discovery`] that uses
/// [`SystemPortDiscovery`].
pub fn run_force_fastboot(options: &ForceFastbootOptions) -> Result<(), ForceFastbootError> {
    run_force_fastboot_with_discovery(options, &SystemPortDiscovery)
}

/// Detect an MTK preloader serial port (via `--port` or by waiting for a new device),
/// handle permission issues with optional udev installation, then perform the fastboot
/// handshake protocol.
pub fn run_force_fastboot_with_discovery(
    options: &ForceFastbootOptions,
    discovery: &dyn PortDiscovery,
) -> Result<(), ForceFastbootError> {
    if permissions::is_running_as_root() && !cfg!(windows) {
        eprintln!(
            "{}",
simple_notice_box(
                Tone::Warning,
                "root execution",
                "Running the whole script as root is unnecessary. Prefer a normal user when possible."
            )
        );
    }

    println!("{}", simple_banner("FORCE FASTBOOT"));
    let auto_udev = !options.no_auto_udev;
    let candidate = if let Some(port) = &options.port {
        serial::candidate_for_device(port, discovery)
    } else {
        serial::wait_for_port(discovery, auto_udev)
            .map_err(|e| ForceFastbootError::NoDevice(format!("{e:#}")))?
    };

    println!(
        "{}",
simple_status_line(Tone::Info, "port", &format!("opening {}", candidate.device))
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
simple_status_line(Tone::Success, "handshake", "FASTBOOT command sent")
    );
    Ok(())
}
