use std::fmt;

use thiserror::Error;

/// NUSB based fastboot client implementation.
pub mod nusb;

use self::nusb::{
    DataDownload as NusbDataDownload, NusbFastBoot, NusbFastBootError, NusbFastBootOpenError,
};

/// Fastboot transport backend kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// nusb (libusb) backend.
    Nusb,
}

impl BackendKind {
    /// Human-readable backend name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nusb => "nusb",
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
}

pub(crate) fn is_missing_variable_message(message: &str) -> bool {
    message.to_ascii_lowercase().contains("variable not found")
}

/// Errors when opening a fastboot device.
#[derive(Debug, Error)]
pub enum FastbootOpenError {
    /// nusb backend open error.
    #[error("nusb: {0}")]
    Nusb(#[from] NusbFastBootOpenError),
}

/// A connected fastboot device with an abstracted backend.
pub struct FastbootDevice {
    backend: NusbFastBoot,
}

/// A download handle for streaming data to a fastboot device.
pub enum DataDownload<'a> {
    /// nusb-based download.
    Nusb(NusbDataDownload<'a>),
}

macro_rules! delegate_device_backend {
    ($backend:expr, $method:ident $(, $args:expr)*) => {{
        $backend.$method($($args),*).await.map_err(FastbootError::from)
    }};
}

macro_rules! delegate_download_open_backend {
    ($backend:expr, $size:expr) => {{
        $backend
            .download($size)
            .await
            .map(DataDownload::Nusb)
            .map_err(FastbootError::from)
    }};
}

macro_rules! delegate_download_handle_backend {
    ($download:expr, $method:ident $(, $args:expr)*) => {{
        match $download {
            DataDownload::Nusb(download) => download
                .$method($($args),*)
                .await
                .map_err(|error| FastbootError::Download(error.to_string())),
        }
    }};
}

impl FastbootDevice {
    /// Return the active backend kind.
    pub fn backend_kind(&self) -> BackendKind {
        BackendKind::Nusb
    }

    /// Query a fastboot variable by name.
    pub async fn get_var(&mut self, var: &str) -> Result<String, FastbootError> {
        delegate_device_backend!(&mut self.backend, get_var, var)
    }

    /// Query a fastboot variable by name, returning `Ok(None)` when the
    /// device reports that the variable does not exist.
    pub async fn get_var_optional(&mut self, var: &str) -> Result<Option<String>, FastbootError> {
        delegate_device_backend!(&mut self.backend, get_var_optional, var)
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
        delegate_device_backend!(&mut self.backend, resize_logical_partition, partition, size)
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
        }
    }

    /// Data left to be sent.
    pub fn left(&self) -> u32 {
        match self {
            Self::Nusb(download) => download.left(),
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
    open_fastboot_with_preferred_backend(None, observer).await
}

/// Open the first available fastboot device, preferring one backend before the other.
pub async fn open_fastboot_with_preferred_backend(
    _preferred_backend: Option<BackendKind>,
    observer: impl FnMut(ProbeEvent),
) -> Result<FastbootDevice, FastbootOpenError> {
    let mut observer = observer;

    let backend = BackendKind::Nusb;
    observer(ProbeEvent {
        backend,
        level: ProbeLogLevel::Info,
        stage: "backend_attempt",
        message: format!("Trying {} backend", backend.as_str()),
    });

    match nusb::open_first_fastboot().await {
        Ok(device) => {
            observer(ProbeEvent {
                backend,
                level: ProbeLogLevel::Info,
                stage: "backend_success",
                message: "Opened fastboot device with nusb".to_string(),
            });
            Ok(FastbootDevice { backend: device })
        }
        Err(error) => {
            observer(ProbeEvent {
                backend,
                level: ProbeLogLevel::Warning,
                stage: "backend_failed",
                message: error.to_string(),
            });
            Err(FastbootOpenError::Nusb(error))
        }
    }
}

/// Return the order in which backends should be probed.
pub fn backend_attempt_order(_preferred_backend: Option<BackendKind>) -> Vec<BackendKind> {
    vec![BackendKind::Nusb]
}

/// Return the alternate backend when one exists.
pub fn alternate_backend_kind(_kind: BackendKind) -> Option<BackendKind> {
    None
}

impl fmt::Debug for FastbootDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FastbootDevice")
            .field("backend", &self.backend_kind().as_str())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{alternate_backend_kind, backend_attempt_order, BackendKind};

    #[test]
    fn backend_attempt_order_defaults_to_nusb() {
        assert_eq!(backend_attempt_order(None), vec![BackendKind::Nusb]);
        assert_eq!(
            backend_attempt_order(Some(BackendKind::Nusb)),
            vec![BackendKind::Nusb]
        );
    }

    #[test]
    fn alternate_backend_kind_is_unavailable() {
        assert_eq!(alternate_backend_kind(BackendKind::Nusb), None);
    }
}
