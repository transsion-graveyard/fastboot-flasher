use std::{fmt, path::PathBuf};

use thiserror::Error;

/// Windows AdbWinApi based fastboot client implementation.
#[cfg(windows)]
pub mod adbwinapi;
/// NUSB based fastboot client implementation.
pub mod nusb;

#[cfg(windows)]
use self::adbwinapi::{
    AdbWinApiFastboot, AdbWinApiFastbootError, AdbWinApiFastbootOpenError, AdbWinApiProbeDetail,
};
use self::nusb::{
    DataDownload as NusbDataDownload, NusbFastBoot, NusbFastBootError, NusbFastBootOpenError,
};

/// Fastboot transport backend kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// nusb (libusb) backend — cross-platform.
    Nusb,
    /// Windows AdbWinApi backend.
    #[cfg(windows)]
    AdbWinApi,
}

impl BackendKind {
    /// Human-readable backend name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nusb => "nusb",
            #[cfg(windows)]
            Self::AdbWinApi => "adbwinapi",
        }
    }
}

/// Log severity for device probe events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeLogLevel {
    /// Informational message.
    Info,
    /// Non-fatal warning.
    Warning,
    /// Error condition.
    Error,
}

/// A device probe event emitted during backend discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeEvent {
    /// The backend that produced this event.
    pub backend: BackendKind,
    /// Severity level.
    pub level: ProbeLogLevel,
    /// Probe stage identifier (e.g. `"backend_attempt"`, `"backend_success"`).
    pub stage: &'static str,
    /// Human-readable event description.
    pub message: String,
}

/// Fastboot communication errors.
#[derive(Debug, Error)]
pub enum FastbootError {
    /// nusb backend error.
    #[error(transparent)]
    Nusb(#[from] NusbFastBootError),
    /// Download transfer error.
    #[error("Download error: {0}")]
    Download(String),
    /// Windows AdbWinApi backend error.
    #[cfg(windows)]
    #[error(transparent)]
    AdbWinApi(#[from] AdbWinApiFastbootError),
}

/// Errors when opening a fastboot device.
#[derive(Debug, Error)]
pub enum FastbootOpenError {
    /// nusb backend open error.
    #[error("nusb: {0}")]
    Nusb(#[from] NusbFastBootOpenError),
    /// Windows AdbWinApi backend open error.
    #[cfg(windows)]
    #[error("adbwinapi: {0}")]
    AdbWinApi(#[from] AdbWinApiFastbootOpenError),
    /// All backends failed (Windows only).
    #[cfg(windows)]
    #[error("no usable fastboot backend found (nusb: {nusb}; adbwinapi: {adbwinapi})")]
    Combined {
        /// nusb error description.
        nusb: String,
        /// AdbWinApi error description.
        adbwinapi: String,
    },
    /// No fastboot backend available.
    #[error("no usable fastboot backend found ({0})")]
    Unavailable(String),
}

/// Paths discovered for AdbWinApi DLLs on Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbWinApiDiscovery {
    /// Path to `AdbWinApi.dll`.
    pub adb_win_api: PathBuf,
    /// Optional path to `AdbWinUsbApi.dll`.
    pub adb_win_usb_api: Option<PathBuf>,
}

/// A connected fastboot device with an abstracted backend.
pub struct FastbootDevice {
    backend: FastbootDeviceBackend,
}

enum FastbootDeviceBackend {
    Nusb(NusbFastBoot),
    #[cfg(windows)]
    AdbWinApi(AdbWinApiFastboot),
}

/// A download handle for streaming data to a fastboot device.
pub enum DataDownload<'a> {
    /// nusb-based download.
    Nusb(NusbDataDownload<'a>),
    /// Windows AdbWinApi-based download.
    #[cfg(windows)]
    AdbWinApi(adbwinapi::DataDownload<'a>),
}

macro_rules! delegate_device_backend {
    ($backend:expr, $method:ident $(, $args:expr)*) => {{
        match $backend {
            FastbootDeviceBackend::Nusb(dev) => {
                dev.$method($($args),*).await.map_err(FastbootError::from)
            }
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => {
                dev.$method($($args),*).await.map_err(FastbootError::from)
            }
        }
    }};
}

macro_rules! delegate_download_open_backend {
    ($backend:expr, $size:expr) => {{
        match $backend {
            FastbootDeviceBackend::Nusb(dev) => dev
                .download($size)
                .await
                .map(DataDownload::Nusb)
                .map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev
                .download($size)
                .await
                .map(DataDownload::AdbWinApi)
                .map_err(FastbootError::from),
        }
    }};
}

macro_rules! delegate_download_handle_backend {
    ($download:expr, $method:ident $(, $args:expr)*) => {{
        match $download {
            DataDownload::Nusb(download) => download
                .$method($($args),*)
                .await
                .map_err(|error| FastbootError::Download(error.to_string())),
            #[cfg(windows)]
            DataDownload::AdbWinApi(download) => {
                download.$method($($args),*).await.map_err(FastbootError::from)
            }
        }
    }};
}

impl FastbootDevice {
    /// Return the active backend kind.
    pub fn backend_kind(&self) -> BackendKind {
        match &self.backend {
            FastbootDeviceBackend::Nusb(_) => BackendKind::Nusb,
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(_) => BackendKind::AdbWinApi,
        }
    }

    /// Query a fastboot variable by name.
    pub async fn get_var(&mut self, var: &str) -> Result<String, FastbootError> {
        delegate_device_backend!(&mut self.backend, get_var, var)
    }

    /// Retrieve all fastboot variables (`getvar:all`).
    pub async fn get_all_vars(
        &mut self,
    ) -> Result<std::collections::HashMap<String, String>, FastbootError> {
        delegate_device_backend!(&mut self.backend, get_all_vars)
    }

    /// Return the device `max-download-size` as a parsed byte count.
    pub async fn max_download_size(&mut self) -> Result<u32, FastbootError> {
        delegate_device_backend!(&mut self.backend, max_download_size)
    }

    /// Return the current A/B slot suffix.
    pub async fn current_slot(&mut self) -> Result<String, FastbootError> {
        delegate_device_backend!(&mut self.backend, current_slot)
    }

    /// Prepare a download of a given size.
    pub async fn download(&mut self, size: u32) -> Result<DataDownload<'_>, FastbootError> {
        delegate_download_open_backend!(&mut self.backend, size)
    }

    /// Flash downloaded data to a target partition.
    pub async fn flash(&mut self, target: &str) -> Result<(), FastbootError> {
        delegate_device_backend!(&mut self.backend, flash, target)
    }

    /// Return whether the given partition is logical.
    pub async fn is_logical(&mut self, partition: &str) -> Result<bool, FastbootError> {
        delegate_device_backend!(&mut self.backend, is_logical, partition)
    }

    /// Resize a logical partition to the given byte size.
    pub async fn resize_logical_partition(
        &mut self,
        partition: &str,
        size: u64,
    ) -> Result<(), FastbootError> {
        delegate_device_backend!(
            &mut self.backend,
            resize_logical_partition,
            partition,
            size
        )
    }

    /// Continue booting the device.
    pub async fn continue_boot(&mut self) -> Result<(), FastbootError> {
        delegate_device_backend!(&mut self.backend, continue_boot)
    }

    /// Set the active A/B slot.
    pub async fn set_active(&mut self, slot: &str) -> Result<(), FastbootError> {
        delegate_device_backend!(&mut self.backend, set_active, slot)
    }

    /// Erase a target partition.
    pub async fn erase(&mut self, target: &str) -> Result<(), FastbootError> {
        delegate_device_backend!(&mut self.backend, erase, target)
    }

    /// Reboot the device.
    pub async fn reboot(&mut self) -> Result<(), FastbootError> {
        delegate_device_backend!(&mut self.backend, reboot)
    }

    /// Reboot the device into the bootloader.
    pub async fn reboot_bootloader(&mut self) -> Result<(), FastbootError> {
        delegate_device_backend!(&mut self.backend, reboot_bootloader)
    }

    /// Power off the device.
    pub async fn power_down(&mut self) -> Result<(), FastbootError> {
        delegate_device_backend!(&mut self.backend, power_down)
    }

    /// Reboot the device into a specific mode.
    pub async fn reboot_to(&mut self, mode: &str) -> Result<(), FastbootError> {
        delegate_device_backend!(&mut self.backend, reboot_to, mode)
    }

    /// Reboot the device directly into fastboot mode.
    pub async fn reboot_fastboot(&mut self) -> Result<(), FastbootError> {
        delegate_device_backend!(&mut self.backend, reboot_fastboot)
    }

    /// Unlock the bootloader via `flashing unlock`.
    pub async fn unlock_bootloader(&mut self) -> Result<(), FastbootError> {
        delegate_device_backend!(&mut self.backend, unlock_bootloader)
    }

    /// Lock the bootloader via `flashing lock`.
    pub async fn lock_bootloader(&mut self) -> Result<(), FastbootError> {
        delegate_device_backend!(&mut self.backend, lock_bootloader)
    }
}

impl<'a> DataDownload<'a> {
    /// Total size of the data transfer.
    pub fn size(&self) -> u32 {
        match self {
            Self::Nusb(download) => download.size(),
            #[cfg(windows)]
            Self::AdbWinApi(download) => download.size(),
        }
    }

    /// Data left to be sent.
    pub fn left(&self) -> u32 {
        match self {
            Self::Nusb(download) => download.left(),
            #[cfg(windows)]
            Self::AdbWinApi(download) => download.left(),
        }
    }

    /// Queue data from a slice for download.
    pub async fn extend_from_slice(&mut self, data: &[u8]) -> Result<(), FastbootError> {
        delegate_download_handle_backend!(self, extend_from_slice, data)
    }

    /// Obtain a mutable buffer to fill with download data.
    pub async fn get_mut_data(&mut self, max: usize) -> Result<&mut [u8], FastbootError> {
        delegate_download_handle_backend!(self, get_mut_data, max)
    }

    /// Finish the download and validate the transfer.
    pub async fn finish(self) -> Result<(), FastbootError> {
        delegate_download_handle_backend!(self, finish)
    }
}

/// Open the first available fastboot device.
pub async fn open_fastboot() -> Result<FastbootDevice, FastbootOpenError> {
    open_fastboot_with_observer(|_| {}).await
}

/// Open the first available fastboot device with a probe observer callback.
pub async fn open_fastboot_with_observer(
    observer: impl FnMut(ProbeEvent),
) -> Result<FastbootDevice, FastbootOpenError> {
    let mut observer = observer;
    observer(ProbeEvent {
        backend: BackendKind::Nusb,
        level: ProbeLogLevel::Info,
        stage: "backend_attempt",
        message: "Trying nusb backend".to_string(),
    });
    match nusb::open_first_fastboot().await {
        Ok(device) => {
            observer(ProbeEvent {
                backend: BackendKind::Nusb,
                level: ProbeLogLevel::Info,
                stage: "backend_success",
                message: "Opened fastboot device with nusb".to_string(),
            });
            Ok(FastbootDevice {
                backend: FastbootDeviceBackend::Nusb(device),
            })
        }
        Err(error) => {
            observer(ProbeEvent {
                backend: BackendKind::Nusb,
                level: ProbeLogLevel::Warning,
                stage: "backend_failed",
                message: error.to_string(),
            });
            #[cfg(not(windows))]
            return Err(FastbootOpenError::Nusb(error));
            #[cfg(windows)]
            {
                observer(ProbeEvent {
                    backend: BackendKind::AdbWinApi,
                    level: ProbeLogLevel::Info,
                    stage: "backend_attempt",
                    message: "Trying AdbWinApi fallback backend".to_string(),
                });
                match AdbWinApiFastboot::open_first() {
                    Ok(device) => {
                        observer(ProbeEvent {
                            backend: BackendKind::AdbWinApi,
                            level: ProbeLogLevel::Info,
                            stage: "backend_success",
                            message: format!(
                                "Opened fastboot device with AdbWinApi from {}",
                                device.discovery().adb_win_api.display()
                            ),
                        });
                        Ok(FastbootDevice {
                            backend: FastbootDeviceBackend::AdbWinApi(device),
                        })
                    }
                    Err(adb_error) => {
                        log_adbwinapi_probe_detail(&mut observer, &adb_error);
                        Err(FastbootOpenError::Combined {
                            nusb: error.to_string(),
                            adbwinapi: adb_error.to_string(),
                        })
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
fn log_adbwinapi_probe_detail(
    observer: &mut impl FnMut(ProbeEvent),
    error: &AdbWinApiFastbootOpenError,
) {
    if let Some(detail) = error.detail() {
        let (level, stage) = match detail {
            AdbWinApiProbeDetail::DllMissing { .. } => (ProbeLogLevel::Warning, "dll_missing"),
            AdbWinApiProbeDetail::DllLoadFailed { .. } => (ProbeLogLevel::Error, "dll_load_failed"),
            AdbWinApiProbeDetail::EnumeratingInterfaces { .. } => {
                (ProbeLogLevel::Info, "enumerating_interfaces")
            }
            AdbWinApiProbeDetail::AndroidInterfaceFound { .. } => {
                (ProbeLogLevel::Info, "android_interface_found")
            }
            AdbWinApiProbeDetail::FastbootInterfaceFound { .. } => {
                (ProbeLogLevel::Info, "fastboot_interface_found")
            }
            AdbWinApiProbeDetail::OpenInterfaceFailed { .. } => {
                (ProbeLogLevel::Warning, "open_interface_failed")
            }
            AdbWinApiProbeDetail::NoAndroidInterface => {
                (ProbeLogLevel::Warning, "no_android_interface")
            }
            AdbWinApiProbeDetail::NoFastbootInterface => {
                (ProbeLogLevel::Warning, "no_fastboot_interface")
            }
        };
        observer(ProbeEvent {
            backend: BackendKind::AdbWinApi,
            level,
            stage,
            message: detail.to_string(),
        });
    }
}

impl fmt::Debug for FastbootDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FastbootDevice")
            .field("backend", &self.backend_kind().as_str())
            .finish()
    }
}
