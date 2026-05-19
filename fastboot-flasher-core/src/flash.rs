//! Flash and erase helpers for single partitions.

use std::path::Path;

use anyhow::Context;
use fastboot_rs::{
    flash_prepared_image, prepare_image, FastbootDevice, FastbootError, FastbootExecutionError,
    FlashProgress,
};

/// Check whether a [`FastbootExecutionError`] represents a "fastboot
/// command failed" response that the caller can safely skip.
pub fn should_skip_failed_partition(err: &FastbootExecutionError) -> bool {
    match err {
        FastbootExecutionError::Fastboot(error) => is_fastboot_failed(error),
        _ => false,
    }
}

fn is_fastboot_failed(err: &FastbootError) -> bool {
    match err {
        FastbootError::Nusb(fastboot_rs::transport::nusb::NusbFastBootError::FastbootFailed(_)) => {
            true
        }
        #[cfg(windows)]
        FastbootError::AdbWinApi(
            fastboot_rs::transport::adbwinapi::AdbWinApiFastbootError::FastbootFailed(_),
        ) => true,
        _ => false,
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
    eprintln!(
        "fastboot flash failed: {partition}: {err}"
    );
    if yes {
        return Ok(true);
    }
    Ok(inquire::Confirm::new(&format!("Skip {partition} and continue?"))
        .with_default(true)
        .prompt()?)
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
    eprintln!(
        "fastboot erase failed: {partition}: {err}"
    );
    if yes {
        return Ok(true);
    }
    Ok(inquire::Confirm::new(&format!("Skip {partition} and continue?"))
        .with_default(true)
        .prompt()?)
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
    eprintln!(
        "[flash-lib] flash_one_partition start partition={} image={} max_download=0x{:x}",
        partition,
        image.display(),
        max_download
    );
    if max_download == 0 {
        anyhow::bail!("device reported max-download-size=0, cannot flash {partition}");
    }
    let prepared = prepare_image(image, max_download)
        .with_context(|| format!("prepare image for {partition}"))?;
    eprintln!(
        "[flash-lib] prepared partition={} transfers={} expanded_size={} file_size={}",
        partition,
        prepared.transfer_count(),
        prepared.expanded_size,
        prepared.file_size
    );
    eprintln!("[flash-lib] querying is-logical for partition={partition}");
    if dev
        .is_logical(partition)
        .await
        .map_err(anyhow::Error::from)
        .with_context(|| format!("query logical partition state for {partition}"))?
    {
        eprintln!(
            "[flash-lib] resizing logical partition={} expanded_size={}",
            partition, prepared.expanded_size
        );
        dev.resize_logical_partition(partition, prepared.expanded_size)
            .await
            .map_err(anyhow::Error::from)
            .with_context(|| {
                format!(
                    "resize logical partition {partition} to {} bytes",
                    prepared.expanded_size
                )
            })?;
    }
    eprintln!("[flash-lib] starting flash_prepared_image partition={partition}");
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