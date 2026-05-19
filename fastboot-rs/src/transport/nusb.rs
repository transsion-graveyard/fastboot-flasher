use nusb::descriptors::TransferType;
use nusb::transfer::Bulk;
use nusb::transfer::Direction;
use nusb::transfer::{Buffer, In, Out};
use nusb::Endpoint;
pub use nusb::{transfer::TransferError, Device, DeviceInfo, Interface};
use std::{collections::HashMap, fmt::Display};
use thiserror::Error;
use tracing::{info, warn};
use tracing::{instrument, trace};

use crate::protocol::{FastBootCommand, FastBootResponse, FastBootResponseParseError};

/// List fastboot devices
pub async fn devices() -> Result<impl Iterator<Item = DeviceInfo>, nusb::Error> {
    Ok(nusb::list_devices()
        .await?
        .filter(|d| NusbFastBoot::find_fastboot_interface(d).is_some()))
}

/// Open the first fastboot device visible to nusb.
pub async fn open_first_fastboot() -> Result<NusbFastBoot, NusbFastBootOpenError> {
    let mut infos = devices().await.map_err(NusbFastBootOpenError::Device)?;
    let info = infos
        .next()
        .ok_or(NusbFastBootOpenError::MissingInterface)?;
    NusbFastBoot::from_info(&info).await
}

/// Fastboot communication errors
#[derive(Debug, Error)]
pub enum NusbFastBootError {
    #[error("Transfer error: {0}")]
    Transfer(#[from] TransferError),
    #[error("Fastboot client failure: {0}")]
    FastbootFailed(String),
    #[error("Unexpected fastboot response")]
    FastbootUnexpectedReply,
    #[error("Unknown fastboot response: {0}")]
    FastbootParseError(#[from] FastBootResponseParseError),
    #[error("Invalid fastboot variable {name}={value:?}: {reason}")]
    InvalidVariable {
        name: &'static str,
        value: String,
        reason: String,
    },
}

/// Errors when opening the fastboot device
#[derive(Debug, Error)]
pub enum NusbFastBootOpenError {
    #[error("Failed to open device: {0}")]
    Device(nusb::Error),
    #[error("Failed to claim interface: {0}")]
    Interface(nusb::Error),
    #[error("Failed to find interface for fastboot")]
    MissingInterface,
    #[error("Failed to find required endpoints for fastboot")]
    MissingEndpoints,
    #[error("Unknown fastboot response: {0}")]
    FastbootParseError(#[from] FastBootResponseParseError),
}

impl NusbFastBootOpenError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Device(_) | Self::Interface(_) | Self::MissingInterface | Self::MissingEndpoints
        )
    }
}

/// Nusb fastboot client
pub struct NusbFastBoot {
    ep_out: Endpoint<Bulk, Out>,
    max_out: usize,
    ep_in: Endpoint<Bulk, In>,
    max_in: usize,
}

impl NusbFastBoot {
    fn parse_is_logical_value(value: &str) -> Result<bool, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "yes" => Ok(true),
            "no" => Ok(false),
            other => Err(format!("unsupported logical partition value: {other}")),
        }
    }

    /// Find fastboot interface within a USB device
    pub fn find_fastboot_interface(info: &DeviceInfo) -> Option<u8> {
        info.interfaces().find_map(|i| {
            if i.class() == 0xff && i.subclass() == 0x42 && i.protocol() == 0x3 {
                Some(i.interface_number())
            } else {
                None
            }
        })
    }

    /// Create a fastboot client based on a USB interface. Interface is assumed to be a fastboot
    /// interface
    #[tracing::instrument(skip_all, err)]
    pub fn from_interface(interface: Interface) -> Result<Self, NusbFastBootOpenError> {
        let (ep_out, max_out, ep_in, max_in) = interface
            .descriptors()
            .find_map(|alt| {
                // Requires one bulk IN and one bulk OUT
                let (ep_out, max_out) = alt.endpoints().find_map(|end| {
                    if end.transfer_type() == TransferType::Bulk
                        && end.direction() == Direction::Out
                    {
                        Some((end.address(), end.max_packet_size()))
                    } else {
                        None
                    }
                })?;

                let (ep_in, max_in) = alt.endpoints().find_map(|end| {
                    if end.transfer_type() == TransferType::Bulk && end.direction() == Direction::In
                    {
                        Some((end.address(), end.max_packet_size()))
                    } else {
                        None
                    }
                })?;
                Some((ep_out, max_out, ep_in, max_in))
            })
            .ok_or(NusbFastBootOpenError::MissingEndpoints)?;
        trace!(
            "Fastboot endpoints: OUT: {} (max: {}), IN: {} (max: {})",
            ep_out,
            max_out,
            ep_in,
            max_in
        );
        let ep_out = interface
            .endpoint::<Bulk, Out>(ep_out)
            .map_err(NusbFastBootOpenError::Interface)?;
        let ep_in = interface
            .endpoint::<Bulk, In>(ep_in)
            .map_err(NusbFastBootOpenError::Interface)?;
        Ok(Self {
            ep_out,
            max_out,
            ep_in,
            max_in,
        })
    }

    /// Create a fastboot client based on a USB device. Interface number must be the fastboot
    /// interface
    #[tracing::instrument(skip_all, err)]
    pub async fn from_device(device: Device, interface: u8) -> Result<Self, NusbFastBootOpenError> {
        let interface = device
            .claim_interface(interface)
            .await
            .map_err(NusbFastBootOpenError::Interface)?;
        Self::from_interface(interface)
    }

    /// Create a fastboot client based on device info. The correct interface will automatically be
    /// determined
    #[tracing::instrument(skip_all, err)]
    pub async fn from_info(info: &DeviceInfo) -> Result<Self, NusbFastBootOpenError> {
        let interface =
            Self::find_fastboot_interface(info).ok_or(NusbFastBootOpenError::MissingInterface)?;
        let device = info.open().await.map_err(NusbFastBootOpenError::Device)?;
        Self::from_device(device, interface).await
    }

    #[tracing::instrument(skip_all, err)]
    async fn send_data(&mut self, data: Vec<u8>) -> Result<(), NusbFastBootError> {
        self.ep_out.submit(data.into());
        self.ep_out.next_complete().await.into_result()?;
        Ok(())
    }

    async fn send_command<S: Display>(
        &mut self,
        cmd: FastBootCommand<S>,
    ) -> Result<(), NusbFastBootError> {
        let out = format!("{}", cmd).into_bytes();
        trace!(
            "Sending command: {}",
            std::str::from_utf8(&out).unwrap_or("Invalid utf-8")
        );
        self.send_data(out).await
    }

    #[tracing::instrument(skip_all, err)]
    async fn read_response(&mut self) -> Result<FastBootResponse, NusbFastBootError> {
        self.ep_in.submit(Buffer::new(self.max_in));
        let resp = self
            .ep_in
            .next_complete()
            .await
            .into_result()
            .map_err(NusbFastBootError::Transfer)?;
        Ok(FastBootResponse::from_bytes(&resp)?)
    }

    #[tracing::instrument(skip_all, err)]
    async fn handle_responses(&mut self) -> Result<String, NusbFastBootError> {
        loop {
            let resp = self.read_response().await?;
            trace!("Response: {:?}", resp);
            match resp {
                FastBootResponse::Info(_) => (),
                FastBootResponse::Text(_) => (),
                FastBootResponse::Data(_) => {
                    return Err(NusbFastBootError::FastbootUnexpectedReply)
                }
                FastBootResponse::Okay(value) => return Ok(value),
                FastBootResponse::Fail(fail) => {
                    return Err(NusbFastBootError::FastbootFailed(fail))
                }
            }
        }
    }

    #[tracing::instrument(skip_all, err)]
    async fn execute<S: Display>(
        &mut self,
        cmd: FastBootCommand<S>,
    ) -> Result<String, NusbFastBootError> {
        self.send_command(cmd).await?;
        self.handle_responses().await
    }

    fn allocate(&self) -> Buffer {
        // Allocate about 1Mb of buffer ensuring it's always a multiple of the maximum out packet
        // size
        let size = (1024usize * 1024).next_multiple_of(self.max_out);
        self.ep_out.allocate(size)
    }

    /// Get the named variable
    ///
    /// The "all" variable is special; For that [Self::get_all_vars] should be used instead
    pub async fn get_var(&mut self, var: &str) -> Result<String, NusbFastBootError> {
        eprintln!("[nusb-fastboot] get_var start var={var}");
        let cmd = FastBootCommand::GetVar(var);
        let result = self.execute(cmd).await;
        match &result {
            Ok(value) => eprintln!("[nusb-fastboot] get_var ok var={} value={}", var, value),
            Err(error) => eprintln!("[nusb-fastboot] get_var err var={} error={}", var, error),
        }
        result
    }

    /// Return the device `max-download-size` as a parsed byte count.
    pub async fn max_download_size(&mut self) -> Result<u32, NusbFastBootError> {
        let value = self.get_var("max-download-size").await?;
        crate::operation::parse_max_download_size(&value).map_err(|err| {
            NusbFastBootError::InvalidVariable {
                name: "max-download-size",
                value,
                reason: err.to_string(),
            }
        })
    }

    /// Return the current A/B slot suffix.
    pub async fn current_slot(&mut self) -> Result<String, NusbFastBootError> {
        let value = self.get_var("current-slot").await?;
        match value.trim().trim_start_matches('_').to_lowercase().as_str() {
            "a" => Ok("a".to_string()),
            "b" => Ok("b".to_string()),
            other => Err(NusbFastBootError::InvalidVariable {
                name: "current-slot",
                value,
                reason: format!("unsupported slot value: {other}"),
            }),
        }
    }

    /// Prepare a download of a given size
    ///
    /// When successful the [DataDownload] helper should be used to actually send the data
    pub async fn download(&'_ mut self, size: u32) -> Result<DataDownload<'_>, NusbFastBootError> {
        eprintln!("[nusb-fastboot] download start size=0x{size:08x}");
        let cmd = FastBootCommand::<&str>::Download(size);
        self.send_command(cmd).await?;
        loop {
            let resp = self.read_response().await?;
            match resp {
                FastBootResponse::Info(i) => info!("info: {i}"),
                FastBootResponse::Text(t) => info!("Text: {}", t),
                FastBootResponse::Data(size) => {
                    eprintln!("[nusb-fastboot] download ready size=0x{size:08x}");
                    return Ok(DataDownload::new(self, size));
                }
                FastBootResponse::Okay(_) => {
                    return Err(NusbFastBootError::FastbootUnexpectedReply)
                }
                FastBootResponse::Fail(fail) => {
                    return Err(NusbFastBootError::FastbootFailed(fail))
                }
            }
        }
    }

    /// Flash downloaded data to a given target partition
    pub async fn flash(&mut self, target: &str) -> Result<(), NusbFastBootError> {
        eprintln!("[nusb-fastboot] flash start target={target}");
        let cmd = FastBootCommand::Flash(target);
        self.execute(cmd).await.map(|v| {
            eprintln!("[nusb-fastboot] flash ok target={} value={}", target, v);
            trace!("Flash ok: {v}");
        })
    }

    /// Return whether the given partition is logical.
    pub async fn is_logical(&mut self, partition: &str) -> Result<bool, NusbFastBootError> {
        eprintln!("[nusb-fastboot] is_logical start partition={partition}");
        let value = match self.get_var(&format!("is-logical:{partition}")).await {
            Ok(value) => value,
            Err(NusbFastBootError::FastbootFailed(message))
                if message.to_ascii_lowercase().contains("variable not found") =>
            {
                eprintln!(
                    "[nusb-fastboot] is_logical missing-var partition={} default=false",
                    partition
                );
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        let parsed = Self::parse_is_logical_value(&value).map_err(|reason| {
            NusbFastBootError::InvalidVariable {
                name: "is-logical",
                value,
                reason,
            }
        })?;
        eprintln!(
            "[nusb-fastboot] is_logical ok partition={} value={}",
            partition, parsed
        );
        Ok(parsed)
    }

    /// Resize a logical partition to the given byte size.
    pub async fn resize_logical_partition(
        &mut self,
        partition: &str,
        size: u64,
    ) -> Result<(), NusbFastBootError> {
        eprintln!(
            "[nusb-fastboot] resize_logical_partition start partition={} size={}",
            partition, size
        );
        let cmd = FastBootCommand::ResizeLogicalPartition { partition, size };
        self.execute(cmd).await.map(|v| {
            eprintln!(
                "[nusb-fastboot] resize_logical_partition ok partition={} value={}",
                partition, v
            );
            trace!("Resize logical partition ok: {v}");
        })
    }

    /// Continue booting
    pub async fn continue_boot(&mut self) -> Result<(), NusbFastBootError> {
        let cmd = FastBootCommand::<&str>::Continue;
        self.execute(cmd).await.map(|v| {
            trace!("Continue ok: {v}");
        })
    }

    /// Set the active A/B slot.
    pub async fn set_active(&mut self, slot: &str) -> Result<(), NusbFastBootError> {
        let cmd = FastBootCommand::SetActive(slot);
        self.execute(cmd).await.map(|v| {
            trace!("Set active ok: {v}");
        })
    }

    /// Erasing the given target partition
    pub async fn erase(&mut self, target: &str) -> Result<(), NusbFastBootError> {
        let cmd = FastBootCommand::Erase(target);
        self.execute(cmd).await.map(|v| {
            trace!("Erase ok: {v}");
        })
    }

    /// Reboot the device
    pub async fn reboot(&mut self) -> Result<(), NusbFastBootError> {
        let cmd = FastBootCommand::<&str>::Reboot;
        self.execute(cmd).await.map(|v| {
            trace!("Reboot ok: {v}");
        })
    }

    /// Reboot the device to fastboot/bootloader mode.
    pub async fn reboot_bootloader(&mut self) -> Result<(), NusbFastBootError> {
        let cmd = FastBootCommand::<&str>::RebootBootloader;
        self.execute(cmd).await.map(|v| {
            trace!("Reboot bootloader ok: {v}");
        })
    }

    /// Power off the device.
    pub async fn power_down(&mut self) -> Result<(), NusbFastBootError> {
        let cmd = FastBootCommand::<&str>::Powerdown;
        self.execute(cmd).await.map(|v| {
            trace!("Powerdown ok: {v}");
        })
    }

    /// Reboot the device to the bootloader
    pub async fn reboot_to(&mut self, mode: &str) -> Result<(), NusbFastBootError> {
        let cmd = FastBootCommand::<&str>::RebootTo(mode);
        self.execute(cmd).await.map(|v| {
            trace!("Reboot ok: {v}");
        })
    }

    /// Reboot the device directly into fastboot mode.
    pub async fn reboot_fastboot(&mut self) -> Result<(), NusbFastBootError> {
        self.reboot_to("fastboot").await
    }

    /// Unlock the bootloader via `flashing unlock`.
    pub async fn unlock_bootloader(&mut self) -> Result<(), NusbFastBootError> {
        let cmd = FastBootCommand::<&str>::FlashingUnlock;
        self.execute(cmd).await.map(|v| {
            trace!("Flashing unlock ok: {v}");
        })
    }

    /// Lock the bootloader via `flashing lock`.
    pub async fn lock_bootloader(&mut self) -> Result<(), NusbFastBootError> {
        let cmd = FastBootCommand::<&str>::FlashingLock;
        self.execute(cmd).await.map(|v| {
            trace!("Flashing lock ok: {v}");
        })
    }

    /// Retrieve all variables
    pub async fn get_all_vars(&mut self) -> Result<HashMap<String, String>, NusbFastBootError> {
        let cmd = FastBootCommand::GetVar("all");
        self.send_command(cmd).await?;
        let mut vars = HashMap::new();
        loop {
            let resp = self.read_response().await?;
            trace!("Response: {:?}", resp);
            match resp {
                FastBootResponse::Info(i) => {
                    let Some((key, value)) = i.rsplit_once(':') else {
                        warn!("Failed to parse variable: {i}");
                        continue;
                    };
                    vars.insert(key.trim().to_string(), value.trim().to_string());
                }
                FastBootResponse::Text(t) => info!("Text: {}", t),
                FastBootResponse::Data(_) => {
                    return Err(NusbFastBootError::FastbootUnexpectedReply)
                }
                FastBootResponse::Okay(_) => {
                    return Ok(vars);
                }
                FastBootResponse::Fail(fail) => {
                    return Err(NusbFastBootError::FastbootFailed(fail))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NusbFastBoot;

    #[test]
    fn parse_is_logical_value_accepts_yes_and_no() {
        assert_eq!(NusbFastBoot::parse_is_logical_value("yes").unwrap(), true);
        assert_eq!(NusbFastBoot::parse_is_logical_value("no").unwrap(), false);
        assert_eq!(NusbFastBoot::parse_is_logical_value(" YES ").unwrap(), true);
    }

    #[test]
    fn parse_is_logical_value_rejects_unknown_values() {
        let error = NusbFastBoot::parse_is_logical_value("maybe").unwrap_err();
        assert!(error.contains("unsupported logical partition value"));
    }
}

/// Error during data download
#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("Trying to complete while nothing was Queued")]
    NothingQueued,
    #[error("Incorrect data length: expected {expected}, got {actual}")]
    IncorrectDataLength { actual: u32, expected: u32 },
    #[error(transparent)]
    Nusb(#[from] NusbFastBootError),
}

/// Data download helper
///
/// To success stream data over usb it needs to be sent in blocks that are multiple of the max
/// endpoint size, otherwise the receiver may complain. It also should only send as much data as
/// was indicate in the DATA command.
///
/// This helper ensures both invariants are met. To do this data needs to be sent by using
/// [DataDownload::extend_from_slice] or [DataDownload::get_mut_data], after sending the data [DataDownload::finish] should be called to
/// validate and finalize.
pub struct DataDownload<'s> {
    fastboot: &'s mut NusbFastBoot,
    size: u32,
    left: u32,
    current: Buffer,
}

impl<'s> DataDownload<'s> {
    fn new(fastboot: &'s mut NusbFastBoot, size: u32) -> DataDownload<'s> {
        let current = fastboot.allocate();
        Self {
            fastboot,
            size,
            left: size,
            current,
        }
    }
}

impl DataDownload<'_> {
    /// Total size of the data transfer
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Data left to be sent/queued
    pub fn left(&self) -> u32 {
        self.left
    }

    /// Extend the streaming from a slice
    ///
    /// This will copy all provided data and send it out if enough is collected. The total amount
    /// of data being sent should not exceed the download size
    pub async fn extend_from_slice(&mut self, mut data: &[u8]) -> Result<(), DownloadError> {
        self.update_size(data.len() as u32)?;
        loop {
            let left = self.current.capacity() - self.current.len();
            if left >= data.len() {
                self.current.extend_from_slice(data);
                break;
            } else {
                self.current.extend_from_slice(&data[0..left]);
                self.next_buffer().await?;
                data = &data[left..];
            }
        }
        Ok(())
    }

    /// This will provide a mutable reference to a [u8] of at most `max` size. The returned slice
    /// should be completely filled with data to be downloaded to the device
    ///
    /// The total amount of data should not exceed the download size
    pub async fn get_mut_data(&mut self, max: usize) -> Result<&mut [u8], DownloadError> {
        if self.current.capacity() == self.current.len() {
            self.next_buffer().await?;
        }

        let left = self.current.capacity() - self.current.len();
        let size = left.min(max);
        self.update_size(size as u32)?;

        let len = self.current.len();
        self.current.extend_fill(size, 0);
        Ok(&mut self.current[len..])
    }

    fn update_size(&mut self, size: u32) -> Result<(), DownloadError> {
        if size > self.left {
            return Err(DownloadError::IncorrectDataLength {
                expected: self.size,
                actual: size - self.left + self.size,
            });
        }
        self.left -= size;
        Ok(())
    }

    async fn next_buffer(&mut self) -> Result<(), DownloadError> {
        let mut next = if self.fastboot.ep_out.pending() < 3 {
            self.fastboot.allocate()
        } else {
            let mut completion = self.fastboot.ep_out.next_complete().await;
            completion.status.map_err(NusbFastBootError::from)?;
            completion.buffer.clear();
            completion.buffer
        };

        std::mem::swap(&mut next, &mut self.current);
        self.fastboot.ep_out.submit(next);

        Ok(())
    }

    /// Finish all pending transfer
    ///
    /// This should only be called if all data has been queued up (matching the total size)
    #[instrument(skip_all, err)]
    pub async fn finish(self) -> Result<(), DownloadError> {
        if self.left != 0 {
            return Err(DownloadError::IncorrectDataLength {
                expected: self.size,
                actual: self.size - self.left,
            });
        }

        if !self.current.is_empty() {
            self.fastboot.ep_out.submit(self.current);
        }

        while self.fastboot.ep_out.pending() > 0 {
            let completion = self.fastboot.ep_out.next_complete().await;
            completion.status.map_err(NusbFastBootError::from)?;
        }

        self.fastboot.handle_responses().await?;
        Ok(())
    }
}
