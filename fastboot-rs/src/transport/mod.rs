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
use self::nusb::{DataDownload as NusbDataDownload, NusbFastBoot, NusbFastBootError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Nusb,
    #[cfg(windows)]
    AdbWinApi,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nusb => "nusb",
            #[cfg(windows)]
            Self::AdbWinApi => "adbwinapi",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeLogLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeEvent {
    pub backend: BackendKind,
    pub level: ProbeLogLevel,
    pub stage: &'static str,
    pub message: String,
}

#[derive(Debug, Error)]
pub enum FastbootError {
    #[error(transparent)]
    Nusb(#[from] NusbFastBootError),
    #[error("Download error: {0}")]
    Download(String),
    #[cfg(windows)]
    #[error(transparent)]
    AdbWinApi(#[from] AdbWinApiFastbootError),
}

#[derive(Debug, Error)]
pub enum FastbootOpenError {
    #[error("nusb: {0}")]
    Nusb(String),
    #[cfg(windows)]
    #[error("adbwinapi: {0}")]
    AdbWinApi(String),
    #[cfg(windows)]
    #[error("no usable fastboot backend found (nusb: {nusb}; adbwinapi: {adbwinapi})")]
    Combined { nusb: String, adbwinapi: String },
    #[error("no usable fastboot backend found ({0})")]
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbWinApiDiscovery {
    pub adb_win_api: PathBuf,
    pub adb_win_usb_api: Option<PathBuf>,
}

pub struct FastbootDevice {
    backend: FastbootDeviceBackend,
}

enum FastbootDeviceBackend {
    Nusb(NusbFastBoot),
    #[cfg(windows)]
    AdbWinApi(AdbWinApiFastboot),
}

pub enum DataDownload<'a> {
    Nusb(NusbDataDownload<'a>),
    #[cfg(windows)]
    AdbWinApi(adbwinapi::DataDownload<'a>),
}

impl FastbootDevice {
    pub fn backend_kind(&self) -> BackendKind {
        match &self.backend {
            FastbootDeviceBackend::Nusb(_) => BackendKind::Nusb,
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(_) => BackendKind::AdbWinApi,
        }
    }

    pub async fn get_var(&mut self, var: &str) -> Result<String, FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.get_var(var).await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.get_var(var).await.map_err(FastbootError::from),
        }
    }

    pub async fn get_all_vars(&mut self) -> Result<std::collections::HashMap<String, String>, FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.get_all_vars().await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.get_all_vars().await.map_err(FastbootError::from),
        }
    }

    pub async fn max_download_size(&mut self) -> Result<u32, FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.max_download_size().await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.max_download_size().await.map_err(FastbootError::from),
        }
    }

    pub async fn current_slot(&mut self) -> Result<String, FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.current_slot().await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.current_slot().await.map_err(FastbootError::from),
        }
    }

    pub async fn download(&mut self, size: u32) -> Result<DataDownload<'_>, FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.download(size).await.map(DataDownload::Nusb).map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.download(size).await.map(DataDownload::AdbWinApi).map_err(FastbootError::from),
        }
    }

    pub async fn flash(&mut self, target: &str) -> Result<(), FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.flash(target).await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.flash(target).await.map_err(FastbootError::from),
        }
    }

    pub async fn is_logical(&mut self, partition: &str) -> Result<bool, FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.is_logical(partition).await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.is_logical(partition).await.map_err(FastbootError::from),
        }
    }

    pub async fn resize_logical_partition(&mut self, partition: &str, size: u64) -> Result<(), FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.resize_logical_partition(partition, size).await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.resize_logical_partition(partition, size).await.map_err(FastbootError::from),
        }
    }

    pub async fn continue_boot(&mut self) -> Result<(), FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.continue_boot().await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.continue_boot().await.map_err(FastbootError::from),
        }
    }

    pub async fn set_active(&mut self, slot: &str) -> Result<(), FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.set_active(slot).await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.set_active(slot).await.map_err(FastbootError::from),
        }
    }

    pub async fn erase(&mut self, target: &str) -> Result<(), FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.erase(target).await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.erase(target).await.map_err(FastbootError::from),
        }
    }

    pub async fn reboot(&mut self) -> Result<(), FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.reboot().await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.reboot().await.map_err(FastbootError::from),
        }
    }

    pub async fn reboot_bootloader(&mut self) -> Result<(), FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.reboot_bootloader().await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.reboot_bootloader().await.map_err(FastbootError::from),
        }
    }

    pub async fn power_down(&mut self) -> Result<(), FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.power_down().await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.power_down().await.map_err(FastbootError::from),
        }
    }

    pub async fn reboot_to(&mut self, mode: &str) -> Result<(), FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.reboot_to(mode).await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.reboot_to(mode).await.map_err(FastbootError::from),
        }
    }

    pub async fn reboot_fastboot(&mut self) -> Result<(), FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.reboot_fastboot().await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.reboot_fastboot().await.map_err(FastbootError::from),
        }
    }

    pub async fn unlock_bootloader(&mut self) -> Result<(), FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.unlock_bootloader().await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.unlock_bootloader().await.map_err(FastbootError::from),
        }
    }

    pub async fn lock_bootloader(&mut self) -> Result<(), FastbootError> {
        match &mut self.backend {
            FastbootDeviceBackend::Nusb(dev) => dev.lock_bootloader().await.map_err(FastbootError::from),
            #[cfg(windows)]
            FastbootDeviceBackend::AdbWinApi(dev) => dev.lock_bootloader().await.map_err(FastbootError::from),
        }
    }
}

impl<'a> DataDownload<'a> {
    pub fn size(&self) -> u32 {
        match self {
            Self::Nusb(download) => download.size(),
            #[cfg(windows)]
            Self::AdbWinApi(download) => download.size(),
        }
    }

    pub fn left(&self) -> u32 {
        match self {
            Self::Nusb(download) => download.left(),
            #[cfg(windows)]
            Self::AdbWinApi(download) => download.left(),
        }
    }

    pub async fn extend_from_slice(&mut self, data: &[u8]) -> Result<(), FastbootError> {
        match self {
            Self::Nusb(download) => download
                .extend_from_slice(data)
                .await
                .map_err(|error| FastbootError::Download(error.to_string())),
            #[cfg(windows)]
            Self::AdbWinApi(download) => download.extend_from_slice(data).await.map_err(FastbootError::from),
        }
    }

    pub async fn get_mut_data(&mut self, max: usize) -> Result<&mut [u8], FastbootError> {
        match self {
            Self::Nusb(download) => download
                .get_mut_data(max)
                .await
                .map_err(|error| FastbootError::Download(error.to_string())),
            #[cfg(windows)]
            Self::AdbWinApi(download) => download.get_mut_data(max).await.map_err(FastbootError::from),
        }
    }

    pub async fn finish(self) -> Result<(), FastbootError> {
        match self {
            Self::Nusb(download) => download
                .finish()
                .await
                .map_err(|error| FastbootError::Download(error.to_string())),
            #[cfg(windows)]
            Self::AdbWinApi(download) => download.finish().await.map_err(FastbootError::from),
        }
    }
}

pub async fn open_fastboot() -> Result<FastbootDevice, FastbootOpenError> {
    open_fastboot_with_observer(|_| {}).await
}

pub async fn open_fastboot_with_observer(
    mut observer: impl FnMut(ProbeEvent),
) -> Result<FastbootDevice, FastbootOpenError> {
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
            return Ok(FastbootDevice {
                backend: FastbootDeviceBackend::Nusb(device),
            });
        }
        Err(error) => {
            observer(ProbeEvent {
                backend: BackendKind::Nusb,
                level: ProbeLogLevel::Warning,
                stage: "backend_failed",
                message: error.to_string(),
            });
            #[cfg(not(windows))]
            return Err(FastbootOpenError::Nusb(error.to_string()));
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
            AdbWinApiProbeDetail::EnumeratingInterfaces { .. } => (ProbeLogLevel::Info, "enumerating_interfaces"),
            AdbWinApiProbeDetail::AndroidInterfaceFound { .. } => (ProbeLogLevel::Info, "android_interface_found"),
            AdbWinApiProbeDetail::FastbootInterfaceFound { .. } => (ProbeLogLevel::Info, "fastboot_interface_found"),
            AdbWinApiProbeDetail::OpenInterfaceFailed { .. } => (ProbeLogLevel::Warning, "open_interface_failed"),
            AdbWinApiProbeDetail::NoAndroidInterface => (ProbeLogLevel::Warning, "no_android_interface"),
            AdbWinApiProbeDetail::NoFastbootInterface => (ProbeLogLevel::Warning, "no_fastboot_interface"),
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
