//! Flash and erase helpers for single partitions.

use std::path::Path;

use anyhow::Context;
use fastboot_rs::{
    flash_prepared_image, prepare_image, FastbootDevice, FastbootError, FastbootExecutionError,
    FlashProgress,
};
use tracing::{debug, warn};

/// Check whether a [`FastbootExecutionError`] represents a "fastboot
/// command failed" response that the caller can safely skip.
pub fn should_skip_failed_partition(err: &FastbootExecutionError) -> bool {
    match err {
        FastbootExecutionError::Fastboot(error) => is_fastboot_failed(error),
        _ => false,
    }
}

/// Check whether an [`anyhow::Error`] contains a fastboot `command failed`
/// response that can be skipped safely.
pub fn should_skip_failed_partition_error(err: &anyhow::Error) -> bool {
    err.chain().any(|source| {
        source
            .downcast_ref::<FastbootExecutionError>()
            .is_some_and(should_skip_failed_partition)
    })
}

fn is_fastboot_failed(err: &FastbootError) -> bool {
    match err {
        FastbootError::Nusb(fastboot_rs::transport::nusb::NusbFastBootError::FastbootFailed(_)) => {
            true
        }
        _ => false,
    }
}

#[allow(async_fn_in_trait)]
trait LogicalPartitionOps {
    async fn is_logical(&mut self, partition: &str) -> Result<bool, FastbootError>;

    async fn resize_logical_partition(
        &mut self,
        partition: &str,
        size: u64,
    ) -> Result<(), FastbootError>;
}

#[allow(async_fn_in_trait)]
impl LogicalPartitionOps for FastbootDevice {
    async fn is_logical(&mut self, partition: &str) -> Result<bool, FastbootError> {
        FastbootDevice::is_logical(self, partition).await
    }

    async fn resize_logical_partition(
        &mut self,
        partition: &str,
        size: u64,
    ) -> Result<(), FastbootError> {
        FastbootDevice::resize_logical_partition(self, partition, size).await
    }
}

async fn resize_logical_partition_if_needed(
    dev: &mut impl LogicalPartitionOps,
    partition: &str,
    expanded_size: u64,
) -> anyhow::Result<bool> {
    debug!(partition, "querying logical partition state");
    if dev
        .is_logical(partition)
        .await
        .map_err(anyhow::Error::from)
        .with_context(|| format!("query logical partition state for {partition}"))?
    {
        debug!(
            partition,
            expanded_size = %expanded_size,
            "resizing logical partition"
        );
        dev.resize_logical_partition(partition, expanded_size)
            .await
            .map_err(anyhow::Error::from)
            .with_context(|| {
                format!(
                    "resize logical partition {partition} to {} bytes",
                    expanded_size
                )
            })?;
        return Ok(true);
    }

    Ok(false)
}

/// Prompt the user (or auto-accept when `yes` is set) whether to skip a
/// partition whose flash failed.
pub fn handle_failed_partition(
    yes: bool,
    partition: &str,
    err: &FastbootExecutionError,
) -> anyhow::Result<bool> {
    if !should_skip_failed_partition(err) {
        return Ok(false);
    }
    warn!(partition, error = %err, "fastboot flash failed");
    if yes {
        return Ok(true);
    }
    Ok(
        inquire::Confirm::new(&format!("Skip {partition} and continue?"))
            .with_default(true)
            .prompt()?,
    )
}

/// Prompt the user (or auto-accept when `yes` is set) whether to skip a
/// partition whose erase failed.
pub fn handle_failed_erase(
    yes: bool,
    partition: &str,
    err: &FastbootError,
) -> anyhow::Result<bool> {
    if !is_fastboot_failed(err) {
        return Ok(false);
    }
    warn!(partition, error = %err, "fastboot erase failed");
    if yes {
        return Ok(true);
    }
    Ok(
        inquire::Confirm::new(&format!("Skip {partition} and continue?"))
            .with_default(true)
            .prompt()?,
    )
}

/// Prepare and flash a single image to a single partition.
/// Handles logical partition resizing and progress callbacks.
pub async fn flash_one_partition(
    dev: &mut FastbootDevice,
    partition: &str,
    image: &Path,
    max_download: u32,
    on_progress: impl FnMut(FlashProgress),
) -> anyhow::Result<()> {
    debug!(
        partition,
        image = %image.display(),
        max_download = %format!("0x{max_download:x}"),
        "flash_one_partition start"
    );
    if max_download == 0 {
        anyhow::bail!("device reported max-download-size=0, cannot flash {partition}");
    }
    let prepared = prepare_image(image, max_download)
        .with_context(|| format!("prepare image for {partition}"))?;
    debug!(
        partition,
        transfers = prepared.transfer_count(),
        expanded_size = prepared.expanded_size,
        file_size = prepared.file_size,
        "prepared image"
    );
    resize_logical_partition_if_needed(dev, partition, prepared.expanded_size).await?;
    debug!(partition, "starting flash_prepared_image");
    flash_prepared_image(dev, partition, &prepared, on_progress)
        .await
        .with_context(|| format!("flash {partition}"))
}

/// Erase a single partition on the device.
pub async fn erase_one_partition(dev: &mut FastbootDevice, partition: &str) -> anyhow::Result<()> {
    dev.erase(partition)
        .await
        .map_err(anyhow::Error::from)
        .with_context(|| format!("erase {partition}"))
}

#[cfg(test)]
mod tests {
    use fastboot_rs::{
        transport::nusb::{NusbFastBootError, TransferError},
        FastbootError, FastbootExecutionError, ImagePayloadError,
    };

    use super::{
        resize_logical_partition_if_needed, should_skip_failed_partition,
        should_skip_failed_partition_error, LogicalPartitionOps,
    };

    fn fastboot_failed_error() -> FastbootExecutionError {
        FastbootExecutionError::Fastboot(FastbootError::Nusb(NusbFastBootError::FastbootFailed(
            "bootloader rejected flash".to_string(),
        )))
    }

    fn non_skippable_error() -> FastbootExecutionError {
        FastbootExecutionError::Payload(ImagePayloadError::SizeTooLarge(1024))
    }

    #[test]
    fn should_skip_failed_partition_accepts_fastboot_failed_responses() {
        assert!(should_skip_failed_partition(&fastboot_failed_error()));
    }

    #[test]
    fn should_skip_failed_partition_rejects_non_fastboot_errors() {
        assert!(!should_skip_failed_partition(&non_skippable_error()));
    }

    #[test]
    fn should_skip_failed_partition_error_finds_wrapped_fastboot_failures() {
        let err = anyhow::Error::new(fastboot_failed_error());

        assert!(should_skip_failed_partition_error(&err));
    }

    #[test]
    fn should_skip_failed_partition_error_rejects_wrapped_non_fastboot_errors() {
        let err = anyhow::Error::new(non_skippable_error());

        assert!(!should_skip_failed_partition_error(&err));
    }

    struct LogicalPartitionOpsMock {
        logical: Option<Result<bool, FastbootError>>,
        calls: Vec<String>,
    }

    impl LogicalPartitionOpsMock {
        fn new(logical: Result<bool, FastbootError>) -> Self {
            Self {
                logical: Some(logical),
                calls: Vec::new(),
            }
        }
    }

    #[allow(async_fn_in_trait)]
    impl LogicalPartitionOps for LogicalPartitionOpsMock {
        async fn is_logical(&mut self, partition: &str) -> Result<bool, FastbootError> {
            self.calls.push(format!("is_logical:{partition}"));
            self.logical.take().expect("logical query should run once")
        }

        async fn resize_logical_partition(
            &mut self,
            partition: &str,
            size: u64,
        ) -> Result<(), FastbootError> {
            self.calls
                .push(format!("resize_logical_partition:{partition}:{size}"));
            Ok(())
        }
    }

    #[tokio::test]
    async fn resize_logical_partition_if_needed_resizes_when_logical() {
        let mut dev = LogicalPartitionOpsMock::new(Ok(true));

        let resized = resize_logical_partition_if_needed(&mut dev, "userdata", 4096)
            .await
            .unwrap();

        assert!(resized);
        assert_eq!(
            dev.calls,
            vec![
                "is_logical:userdata".to_string(),
                "resize_logical_partition:userdata:4096".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn resize_logical_partition_if_needed_skips_when_not_logical() {
        let mut dev = LogicalPartitionOpsMock::new(Ok(false));

        let resized = resize_logical_partition_if_needed(&mut dev, "userdata", 4096)
            .await
            .unwrap();

        assert!(!resized);
        assert_eq!(dev.calls, vec!["is_logical:userdata".to_string()]);
    }

    #[tokio::test]
    async fn resize_logical_partition_if_needed_propagates_query_errors() {
        let mut dev = LogicalPartitionOpsMock::new(Err(FastbootError::Nusb(
            NusbFastBootError::Transfer(TransferError::Fault),
        )));

        let error = resize_logical_partition_if_needed(&mut dev, "userdata", 4096)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("query logical partition state"));
        assert_eq!(dev.calls, vec!["is_logical:userdata".to_string()]);
    }
}
