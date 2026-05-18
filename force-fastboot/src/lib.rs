#![deny(unsafe_code)]

pub mod permissions;
pub mod protocol;
pub mod serial;
pub mod spinner;
pub mod udev;

use serial::{PortDiscovery, SystemPortDiscovery};
use terminal_output::chrome::{banner, notice_box, status_line, Tone};

#[derive(Debug, Clone, Default)]
pub struct ForceFastbootOptions {
    pub port: Option<String>,
    pub no_auto_udev: bool,
}

pub fn run_force_fastboot(options: &ForceFastbootOptions) -> anyhow::Result<()> {
    run_force_fastboot_with_discovery(options, &SystemPortDiscovery)
}

pub fn run_force_fastboot_with_discovery(
    options: &ForceFastbootOptions,
    discovery: &dyn PortDiscovery,
) -> anyhow::Result<()> {
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
        serial::wait_for_port(discovery, auto_udev)?
    };

    println!(
        "{}",
        status_line(Tone::Info, "port", &format!("opening {}", candidate.device))
    );
    let mut port = serial::open_with_permission_recovery(&candidate, discovery, auto_udev)?;

    {
        let _spinner = spinner::StatusSpinner::new("Waiting for preloader handshake byte...");
        protocol::force_fastboot(port.as_mut())?;
    }

    println!(
        "{}",
        status_line(Tone::Success, "handshake", "FASTBOOT command sent")
    );
    Ok(())
}
