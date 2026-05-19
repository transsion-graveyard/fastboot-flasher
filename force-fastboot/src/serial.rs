use std::collections::HashSet;
use std::time::Duration;

use crate::permissions;
use crate::protocol::SerialIo;
use crate::spinner::StatusSpinner;
use crate::udev;

const BAUD: u32 = 115200;
const TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortCandidate {
    pub device: String,
    pub description: String,
    pub hwid: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
}

pub enum PortSearchResult {
    Openable(PortCandidate),
    NothingFound {
        permission_denied: Option<PortCandidate>,
    },
}

pub trait PortDiscovery {
    fn list_candidates(&self) -> Vec<PortCandidate>;
    fn try_open(&self, device: &str) -> Result<(), anyhow::Error>;
    fn open(&self, device: &str) -> Result<Box<dyn SerialIo>, anyhow::Error>;
}

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
