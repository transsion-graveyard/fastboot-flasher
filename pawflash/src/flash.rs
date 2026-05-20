//! Flash and erase helpers for single partitions.

use std::future::Future;
use std::path::Path;

use anyhow::Context;
use fastboot_rs::transport::nusb::NusbFastBootError;
use fastboot_rs::{
    flash_prepared_image, prepare_image, FastbootDevice, FastbootError, FastbootExecutionError,
    FlashProgress, ImagePreparationError,
};
use tracing::{debug, warn};

/// Whether a flash should resize a logical partition before downloading data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeLogicalPartition {
    /// Keep the current partition size.
    Skip,
    /// Query the device and resize if the target partition is logical.
    IfLogical,
}

/// Whether a scatter-flash partition error should be skipped (non-fatal).
///
/// Returns `true` when the error is recoverable – device command rejection,
/// missing/corrupt image files, or payload materialisation issues.  Returns
/// `false` for transport-level failures (USB disconnects, protocol errors)
/// that should abort the entire flash run.
pub fn is_scatter_skippable_error(err: &anyhow::Error) -> bool {
    err.chain().any(|source| {
        // Device rejected the fastboot command – skip safely.
        if let Some(FastbootExecutionError::Fastboot(fe)) =
            source.downcast_ref::<FastbootExecutionError>()
        {
            return matches!(
                fe,
                FastbootError::Nusb(NusbFastBootError::FastbootFailed(_))
            );
        }
        // Payload materialisation failed (I/O, size too large) – skip.
        if matches!(
            source.downcast_ref::<FastbootExecutionError>(),
            Some(FastbootExecutionError::Payload(_))
        ) {
            return true;
        }
        // Image preparation failed (file not found, bad sparse, etc.) – skip.
        if source.downcast_ref::<ImagePreparationError>().is_some() {
            return true;
        }
        false
    })
}

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
    matches!(
        err,
        FastbootError::Nusb(NusbFastBootError::FastbootFailed(_))
    )
}

trait LogicalPartitionOps {
    fn is_logical(
        &mut self,
        partition: &str,
    ) -> impl Future<Output = Result<bool, FastbootError>> + Send;

    fn resize_logical_partition(
        &mut self,
        partition: &str,
        size: u64,
    ) -> impl Future<Output = Result<(), FastbootError>> + Send;
}

impl LogicalPartitionOps for FastbootDevice {
    fn is_logical(
        &mut self,
        partition: &str,
    ) -> impl Future<Output = Result<bool, FastbootError>> + Send {
        FastbootDevice::is_logical(self, partition)
    }

    fn resize_logical_partition(
        &mut self,
        partition: &str,
        size: u64,
    ) -> impl Future<Output = Result<(), FastbootError>> + Send {
        FastbootDevice::resize_logical_partition(self, partition, size)
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

async fn maybe_resize_logical_partition(
    dev: &mut impl LogicalPartitionOps,
    partition: &str,
    expanded_size: u64,
    resize: ResizeLogicalPartition,
) -> anyhow::Result<bool> {
    match resize {
        ResizeLogicalPartition::Skip => Ok(false),
        ResizeLogicalPartition::IfLogical => {
            resize_logical_partition_if_needed(dev, partition, expanded_size).await
        }
    }
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

/// Prepare and flash a single image to a single partition without resizing it.
pub async fn flash_one_partition(
    dev: &mut FastbootDevice,
    partition: &str,
    image: &Path,
    max_download: u32,
    on_progress: impl FnMut(FlashProgress),
) -> anyhow::Result<()> {
    flash_one_partition_with_resize(
        dev,
        partition,
        image,
        max_download,
        ResizeLogicalPartition::Skip,
        on_progress,
    )
    .await
}

/// Prepare and flash a single image to a single partition.
/// Optionally handles logical partition resizing and progress callbacks.
pub async fn flash_one_partition_with_resize(
    dev: &mut FastbootDevice,
    partition: &str,
    image: &Path,
    max_download: u32,
    resize: ResizeLogicalPartition,
    on_progress: impl FnMut(FlashProgress),
) -> anyhow::Result<()> {
    debug!(
        partition,
        image = %image.display(),
        max_download = %format!("0x{max_download:x}"),
        ?resize,
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
    maybe_resize_logical_partition(dev, partition, prepared.expanded_size, resize).await?;
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
        FastbootError, FastbootExecutionError, ImagePayloadError, ImagePreparationError,
    };

    use super::{
        is_scatter_skippable_error, maybe_resize_logical_partition,
        resize_logical_partition_if_needed, should_skip_failed_partition,
        should_skip_failed_partition_error, LogicalPartitionOps, ResizeLogicalPartition,
    };

    fn fastboot_failed_error() -> FastbootExecutionError {
        FastbootExecutionError::Fastboot(FastbootError::Nusb(NusbFastBootError::FastbootFailed(
            "bootloader rejected flash".to_string(),
        )))
    }

    fn non_skippable_error() -> FastbootExecutionError {
        FastbootExecutionError::Payload(ImagePayloadError::SizeTooLarge(1024))
    }

    fn transport_error() -> FastbootExecutionError {
        FastbootExecutionError::Fastboot(FastbootError::Nusb(NusbFastBootError::Transfer(
            TransferError::Fault,
        )))
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

    #[test]
    fn is_scatter_skippable_error_accepts_fastboot_failed() {
        let err = anyhow::Error::new(fastboot_failed_error());
        assert!(is_scatter_skippable_error(&err));
    }

    #[test]
    fn is_scatter_skippable_error_accepts_payload_errors() {
        let err = anyhow::Error::new(FastbootExecutionError::Payload(ImagePayloadError::Io(
            std::io::Error::new(std::io::ErrorKind::NotFound, "no file"),
        )));
        assert!(is_scatter_skippable_error(&err));
    }

    #[test]
    fn is_scatter_skippable_error_accepts_image_preparation_errors() {
        let err = anyhow::Error::new(ImagePreparationError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no file",
        )));
        assert!(is_scatter_skippable_error(&err));
    }

    #[test]
    fn is_scatter_skippable_error_rejects_transport_errors() {
        let err = anyhow::Error::new(transport_error());
        assert!(!is_scatter_skippable_error(&err));
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

    impl LogicalPartitionOps for LogicalPartitionOpsMock {
        fn is_logical(
            &mut self,
            partition: &str,
        ) -> impl std::future::Future<Output = Result<bool, FastbootError>> + Send {
            self.calls.push(format!("is_logical:{partition}"));
            let result = self.logical.take().expect("logical query should run once");
            async move { result }
        }

        fn resize_logical_partition(
            &mut self,
            partition: &str,
            size: u64,
        ) -> impl std::future::Future<Output = Result<(), FastbootError>> + Send {
            self.calls
                .push(format!("resize_logical_partition:{partition}:{size}"));
            async { Ok(()) }
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

    #[tokio::test]
    async fn maybe_resize_logical_partition_skips_query_when_disabled() {
        let mut dev = LogicalPartitionOpsMock::new(Ok(true));

        let resized = maybe_resize_logical_partition(
            &mut dev,
            "system_a",
            4096,
            ResizeLogicalPartition::Skip,
        )
        .await
        .unwrap();

        assert!(!resized);
        assert!(dev.calls.is_empty());
    }

    #[tokio::test]
    async fn maybe_resize_logical_partition_queries_when_enabled() {
        let mut dev = LogicalPartitionOpsMock::new(Ok(true));

        let resized = maybe_resize_logical_partition(
            &mut dev,
            "system_a",
            4096,
            ResizeLogicalPartition::IfLogical,
        )
        .await
        .unwrap();

        assert!(resized);
        assert_eq!(
            dev.calls,
            vec![
                "is_logical:system_a".to_string(),
                "resize_logical_partition:system_a:4096".to_string(),
            ]
        );
    }
}
