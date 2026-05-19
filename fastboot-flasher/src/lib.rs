#![cfg_attr(not(windows), deny(unsafe_code))]
#![deny(missing_docs)]

//! Orchestration helpers for the `fastboot-flasher` CLI and Tauri backend.

pub mod cli;
pub mod device;
pub mod format;
pub mod gsi;
pub mod manual;
pub mod plan;
pub mod progress;

pub use fastboot_rs::{FastbootDevice, FastbootError, FastbootExecutionError, FlashProgress};

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use fastboot_rs::{flash_prepared_image, open_fastboot, parse_max_download_size, prepare_image};
use inquire::Confirm;
use mtk_scatter_parser::FlashPlan;
use terminal_output::chrome::{notice_box, Tone};
use tokio::time::sleep;

use crate::cli::{FlashMode, SlotArg};

/// Check whether a [`FastbootExecutionError`] represents a "fastboot
/// command failed" response that the caller can safely skip.
pub fn should_skip_failed_partition(err: &FastbootExecutionError) -> bool {
    match err {
        FastbootExecutionError::Fastboot(error) => is_fastboot_failed(error),
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
        "{}",
        notice_box(
            Tone::Error,
            "fastboot flash failed",
            &format!("{partition}: {err}")
        )
    );
    if yes {
        return Ok(true);
    }
    Ok(Confirm::new(&format!("Skip {partition} and continue?"))
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
        "{}",
        notice_box(
            Tone::Error,
            "fastboot erase failed",
            &format!("{partition}: {err}")
        )
    );
    if yes {
        return Ok(true);
    }
    Ok(Confirm::new(&format!("Skip {partition} and continue?"))
        .with_default(true)
        .prompt()?)
}

/// Poll `open_fastboot` every 250ms until a device is found.
pub async fn connect_fastboot() -> anyhow::Result<FastbootDevice> {
    loop {
        match open_fastboot().await {
            Ok(dev) => return Ok(dev),
            Err(_) => {
                sleep(Duration::from_millis(250)).await;
            }
        }
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

/// Read a single fastboot variable from the device.
pub async fn read_variable(dev: &mut FastbootDevice, var: &str) -> anyhow::Result<String> {
    dev.get_var(var)
        .await
        .map_err(anyhow::Error::from)
        .with_context(|| format!("get variable {var}"))
}

/// Read all fastboot variables from the device.
pub async fn read_all_variables(
    dev: &mut FastbootDevice,
) -> anyhow::Result<HashMap<String, String>> {
    dev.get_all_vars()
        .await
        .map_err(anyhow::Error::from)
        .context("get all variables")
}

/// Read the `max-download-size` variable from a variables map and parse it.
pub fn resolve_max_download_size_from_vars(vars: &HashMap<String, String>) -> anyhow::Result<u32> {
    let raw = vars
        .get("max-download-size")
        .context("missing fastboot variable max-download-size")?;
    let max_download =
        parse_max_download_size(raw).with_context(|| format!("parse max-download-size `{raw}`"))?;
    if max_download == 0 {
        anyhow::bail!("device reported max-download-size=0");
    }
    Ok(max_download)
}

/// Set the active boot slot on the device.
pub async fn set_fastboot_active_slot(dev: &mut FastbootDevice, slot: &str) -> anyhow::Result<()> {
    dev.set_active(slot)
        .await
        .map_err(anyhow::Error::from)
        .with_context(|| format!("set active slot to {slot}"))
}

/// Reboot the device into the normal OS.
pub async fn reboot_device(dev: &mut FastbootDevice) -> anyhow::Result<()> {
    dev.reboot()
        .await
        .map_err(anyhow::Error::from)
        .context("reboot device")
}

/// Reboot the device into the bootloader.
pub async fn reboot_device_bootloader(dev: &mut FastbootDevice) -> anyhow::Result<()> {
    dev.reboot_bootloader()
        .await
        .map_err(anyhow::Error::from)
        .context("reboot to bootloader")
}

/// Reboot the device into fastbootd (userspace fastboot).
pub async fn reboot_device_fastboot(dev: &mut FastbootDevice) -> anyhow::Result<()> {
    dev.reboot_fastboot()
        .await
        .map_err(anyhow::Error::from)
        .context("reboot to fastboot")
}

/// Power off the device.
pub async fn power_off_device(dev: &mut FastbootDevice) -> anyhow::Result<()> {
    dev.power_down()
        .await
        .map_err(anyhow::Error::from)
        .context("power off device")
}

/// Send the `flashing unlock` command to unlock the bootloader.
pub async fn send_flashing_unlock(dev: &mut FastbootDevice) -> anyhow::Result<()> {
    dev.unlock_bootloader()
        .await
        .map_err(anyhow::Error::from)
        .context("unlock bootloader")
}

/// Send the `flashing lock` command to lock the bootloader.
pub async fn send_flashing_lock(dev: &mut FastbootDevice) -> anyhow::Result<()> {
    dev.lock_bootloader()
        .await
        .map_err(anyhow::Error::from)
        .context("lock bootloader")
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

/// Build a flash plan by parsing a scatter file with the given mode, slot,
/// preloader, and partition filters.
pub fn build_flash_plan(
    scatter_path: &Path,
    mode: FlashMode,
    slot: Option<SlotArg>,
    include_preloader: bool,
    parts: Vec<String>,
    check_images: bool,
) -> anyhow::Result<FlashPlan> {
    crate::plan::build_plan_checked(
        scatter_path,
        mode,
        slot,
        include_preloader,
        parts,
        check_images,
    )
}

/// Run the `force-fastboot` helper to push an MTK device into fastboot mode.
pub fn force_fastboot() -> anyhow::Result<()> {
    force_fastboot::run_force_fastboot(&force_fastboot::ForceFastbootOptions::default())
        .map_err(anyhow::Error::from)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    #[test]
    fn power_off_helper_is_exported() {
        let _ = super::power_off_device;
    }

    #[test]
    fn reboot_fastboot_helper_is_exported() {
        let _ = super::reboot_device_fastboot;
    }

    #[test]
    fn resolve_max_download_size_from_vars_accepts_hex_values() {
        let vars = HashMap::from([("max-download-size".to_string(), "0x4000000".to_string())]);

        let max_download = super::resolve_max_download_size_from_vars(&vars).unwrap();

        assert_eq!(max_download, 0x4000000);
    }

    #[test]
    fn resolve_max_download_size_from_vars_rejects_zero() {
        let vars = HashMap::from([("max-download-size".to_string(), "0".to_string())]);

        let error = super::resolve_max_download_size_from_vars(&vars).unwrap_err();

        assert!(error.to_string().contains("max-download-size=0"));
    }

    #[test]
    fn resolve_max_download_size_from_vars_requires_variable() {
        let error = super::resolve_max_download_size_from_vars(&HashMap::new()).unwrap_err();

        assert!(error.to_string().contains("max-download-size"));
    }
}
