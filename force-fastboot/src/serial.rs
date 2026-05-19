//!
//! Serial port discovery, candidate selection, and opening with permission-recovery logic.
//!
//! Provides the [`PortDiscovery`] trait and [`SystemPortDiscovery`] for enumerating and
//! opening serial ports, plus helper functions for waiting on newly-connected preloader devices.

use std::collections::HashSet;
use std::time::Duration;

use crate::permissions;
use crate::protocol::SerialIo;
use crate::spinner::StatusSpinner;
use crate::udev;

const BAUD: u32 = 115200;
const TIMEOUT: Duration = Duration::from_millis(250);

/// A discovered serial port that may be an MTK preloader device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortCandidate {
    /// Filesystem path to the serial device (e.g. `/dev/ttyACM0`).
    pub device: String,
    /// Human-readable product description from USB descriptors, if available.
    pub description: String,
    /// Hardware identifier (serial number) from the USB device, if available.
    pub hwid: String,
    /// USB vendor ID, if the port is a USB device.
    pub vid: Option<u16>,
    /// USB product ID, if the port is a USB device.
    pub pid: Option<u16>,
}

/// Result of a port scan indicating whether an openable device was found.
pub enum PortSearchResult {
    /// A new device was found and can be opened successfully.
    Openable(PortCandidate),
    /// No new openable devices were found.
    NothingFound {
        /// A device was found but opening it returned a permission error.
        permission_denied: Option<PortCandidate>,
    },
}

/// Abstraction for enumerating and opening serial ports, enabling injection of fake
/// implementations in tests.
pub trait PortDiscovery {
    /// List all available serial ports as [`PortCandidate`]s.
    fn list_candidates(&self) -> Vec<PortCandidate>;
    /// Probe whether `device` can be opened without keeping it open.
    fn try_open(&self, device: &str) -> Result<(), anyhow::Error>;
    /// Open `device` and return a [`SerialIo`] handle.
    fn open(&self, device: &str) -> Result<Box<dyn SerialIo>, anyhow::Error>;
}

/// Real implementation of [`PortDiscovery`] that delegates to the `serialport` crate.
pub struct SystemPortDiscovery;

impl SystemPortDiscovery {
    fn open_port(device: &str) -> Result<Box<dyn SerialIo>, serialport::Error> {
        let port = serialport::new(device, BAUD).timeout(TIMEOUT).open()?;
        Ok(Box::new(RealSerialPort { port }))
    }
}

struct RealSerialPort {
    port: Box<dyn serialport::SerialPort>,
}

impl SerialIo for RealSerialPort {
    fn read_byte(&mut self) -> std::io::Result<Option<u8>> {
        let mut buf = [0u8; 1];
        match self.port.read(&mut buf) {
            Ok(1) => Ok(Some(buf[0])),
            Ok(0) => Ok(None),
            Ok(_) => Ok(Some(buf[0])),
            Err(e) => match e.kind() {
                std::io::ErrorKind::TimedOut => Ok(None),
                _ => Err(e),
            },
        }
    }

    fn flush_input(&mut self) -> std::io::Result<()> {
        self.port.clear(serialport::ClearBuffer::Input)?;
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.port.write_all(buf)?;
        self.port.flush()
    }
}

impl PortDiscovery for SystemPortDiscovery {
    fn list_candidates(&self) -> Vec<PortCandidate> {
        let mut ports: Vec<PortCandidate> = serialport::available_ports()
            .unwrap_or_default()
            .into_iter()
            .map(|info| {
                let (description, hwid, vid, pid) = match info.port_type {
                    serialport::SerialPortType::UsbPort(usb) => (
                        usb.product.unwrap_or_default(),
                        usb.serial_number.unwrap_or_default(),
                        Some(usb.vid),
                        Some(usb.pid),
                    ),
                    _ => (String::new(), String::new(), None, None),
                };
                PortCandidate {
                    device: info.port_name,
                    description,
                    hwid,
                    vid,
                    pid,
                }
            })
            .collect();
        ports.sort_by(|a, b| a.device.cmp(&b.device));
        ports
    }

    fn try_open(&self, device: &str) -> Result<(), anyhow::Error> {
        Self::open_port(device)?;
        Ok(())
    }

    fn open(&self, device: &str) -> Result<Box<dyn SerialIo>, anyhow::Error> {
        Self::open_port(device).map_err(anyhow::Error::new)
    }
}

/// Scan for a newly-connected serial port not in `previous` and report whether it can be opened.
pub fn find_new_port(
    previous: &HashSet<String>,
    discovery: &dyn PortDiscovery,
) -> PortSearchResult {
    let mut permission_denied: Option<PortCandidate> = None;

    for candidate in discovery.list_candidates() {
        if previous.contains(&candidate.device) {
            continue;
        }

        match discovery.try_open(&candidate.device) {
            Ok(()) => return PortSearchResult::Openable(candidate),
            Err(e) if permissions::is_permission_error(&e) => {
                permission_denied = Some(candidate);
                continue;
            }
            Err(_) => continue,
        }
    }

    PortSearchResult::NothingFound { permission_denied }
}

/// Poll for a preloader serial port to appear, installing udev rules or printing
/// permission guidance when a permission-denied device is detected.
pub fn wait_for_port(
    discovery: &dyn PortDiscovery,
    auto_udev: bool,
) -> anyhow::Result<PortCandidate> {
    let previous_devices: HashSet<String> = discovery
        .list_candidates()
        .into_iter()
        .map(|c| c.device)
        .collect();
    let mut tried_udev_for: HashSet<String> = HashSet::new();

    let _spinner = StatusSpinner::new("Waiting for MTK preloader serial port...");

    loop {
        match find_new_port(&previous_devices, discovery) {
            PortSearchResult::Openable(candidate) => return Ok(candidate),
            PortSearchResult::NothingFound {
                permission_denied: Some(denied),
            } => {
                if !tried_udev_for.contains(&denied.device) {
                    tried_udev_for.insert(denied.device.clone());
                    if auto_udev {
                        udev::auto_install_linux_rule(&denied);
                    } else {
                        udev::print_permission_guidance(&denied);
                    }
                }
            }
            PortSearchResult::NothingFound {
                permission_denied: None,
            } => {}
        }

        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Return the [`PortCandidate`] matching `device`, or a fallback with synthetic metadata
/// if the device is not present in the current port list.
pub fn candidate_for_device(device: &str, discovery: &dyn PortDiscovery) -> PortCandidate {
    for candidate in discovery.list_candidates() {
        if candidate.device == device {
            return candidate;
        }
    }
    PortCandidate {
        device: device.to_string(),
        description: "selected by --port".to_string(),
        hwid: String::new(),
        vid: None,
        pid: None,
    }
}

/// Open a device, attempting automatic udev rule installation on Linux if opening
/// fails due to a permission error.
pub fn open_with_permission_recovery(
    candidate: &PortCandidate,
    discovery: &dyn PortDiscovery,
    auto_udev: bool,
) -> anyhow::Result<Box<dyn SerialIo>> {
    match discovery.open(&candidate.device) {
        Ok(port) => Ok(port),
        Err(e) if permissions::is_permission_error(&e) => {
            if auto_udev {
                if udev::auto_install_linux_rule(candidate) {
                    return discovery.open(&candidate.device);
                }
            } else {
                udev::print_permission_guidance(candidate);
            }
            Err(e)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    struct FakeDiscovery {
        ports: Vec<PortCandidate>,
        permission_denied: HashSet<String>,
    }

    impl FakeDiscovery {
        fn new(ports: Vec<PortCandidate>) -> Self {
            Self {
                ports,
                permission_denied: HashSet::new(),
            }
        }

        fn with_permission_denied(mut self, devices: &[&str]) -> Self {
            for d in devices {
                self.permission_denied.insert(d.to_string());
            }
            self
        }
    }

    impl PortDiscovery for FakeDiscovery {
        fn list_candidates(&self) -> Vec<PortCandidate> {
            self.ports.clone()
        }

        fn try_open(&self, device: &str) -> Result<(), anyhow::Error> {
            if self.permission_denied.contains(device) {
                return Err(anyhow::Error::new(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Permission denied",
                )));
            }
            Ok(())
        }

        fn open(&self, _device: &str) -> Result<Box<dyn SerialIo>, anyhow::Error> {
            unimplemented!("not used in these tests")
        }
    }

    #[test]
    fn find_new_port_finds_new_device() {
        let candidate = PortCandidate {
            device: "/dev/ttyACM0".into(),
            description: "Preloader".into(),
            hwid: "USB".into(),
            vid: Some(0x0E8D),
            pid: Some(0x2000),
        };
        let discovery = FakeDiscovery::new(vec![candidate.clone()]);

        let result = find_new_port(&HashSet::new(), &discovery);

        match result {
            PortSearchResult::Openable(c) => assert_eq!(c, candidate),
            _ => panic!("expected Openable"),
        }
    }

    #[test]
    fn find_new_port_skips_known_devices() {
        let candidate = PortCandidate {
            device: "/dev/ttyACM0".into(),
            description: "Preloader".into(),
            hwid: "USB".into(),
            vid: None,
            pid: None,
        };
        let discovery = FakeDiscovery::new(vec![candidate]);
        let previous: HashSet<String> = ["/dev/ttyACM0".into()].into_iter().collect();

        let result = find_new_port(&previous, &discovery);

        match result {
            PortSearchResult::NothingFound {
                permission_denied: None,
            } => {}
            _ => panic!("expected NothingFound"),
        }
    }

    #[test]
    fn find_new_port_reports_permission_candidate() {
        let candidate = PortCandidate {
            device: "/dev/ttyACM0".into(),
            description: "Preloader".into(),
            hwid: "USB".into(),
            vid: Some(0x0E8D),
            pid: Some(0x2000),
        };
        let discovery =
            FakeDiscovery::new(vec![candidate.clone()]).with_permission_denied(&["/dev/ttyACM0"]);

        let result = find_new_port(&HashSet::new(), &discovery);

        match result {
            PortSearchResult::NothingFound {
                permission_denied: Some(denied),
            } => assert_eq!(denied, candidate),
            _ => panic!("expected NothingFound with permission_denied"),
        }
    }

    #[test]
    fn candidate_for_device_returns_fallback_when_not_listed() {
        let discovery = FakeDiscovery::new(vec![]);
        let candidate = candidate_for_device("/dev/ttyUSB0", &discovery);

        assert_eq!(candidate.device, "/dev/ttyUSB0");
        assert_eq!(candidate.description, "selected by --port");
    }
}
