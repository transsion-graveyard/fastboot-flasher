#![deny(unsafe_code)]

//! Orchestration helpers for the `fastboot-flasher` CLI and Tauri backend.

pub mod cli;
pub mod device;
pub mod format;
pub mod manual;
pub mod plan;
pub mod progress;

pub use fastboot_rs::{transport::nusb::NusbFastBoot, FastbootExecutionError, FlashProgress};

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use fastboot_rs::{
    flash_prepared_image, prepare_image,
    transport::nusb::{devices, NusbFastBootError},
};
use inquire::Confirm;
use mtk_scatter_parser::FlashPlan;
use terminal_output::chrome::{notice_box, Tone};
use tokio::time::sleep;

use crate::cli::{FlashMode, SlotArg};

pub fn should_skip_failed_partition(err: &FastbootExecutionError) -> bool {
    matches!(
        err,
        FastbootExecutionError::Fastboot(NusbFastBootError::FastbootFailed(_))
    )
}

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

pub fn handle_failed_erase(
    yes: bool,
    partition: &str,
    err: &NusbFastBootError,
) -> anyhow::Result<bool> {
    if !matches!(err, NusbFastBootError::FastbootFailed(_)) {
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

pub async fn connect_fastboot() -> anyhow::Result<NusbFastBoot> {
    loop {
        let mut infos = devices().await?;
        if let Some(info) = infos.next() {
            return NusbFastBoot::from_info(&info)
                .await
                .map_err(anyhow::Error::from);
        }
        sleep(Duration::from_millis(500)).await;
    }
}

pub async fn read_variable(dev: &mut NusbFastBoot, var: &str) -> anyhow::Result<String> {
    dev.get_var(var)
        .await
        .map_err(anyhow::Error::from)
        .with_context(|| format!("get variable {var}"))
}

pub async fn read_all_variables(dev: &mut NusbFastBoot) -> anyhow::Result<HashMap<String, String>> {
    dev.get_all_vars()
        .await
        .map_err(anyhow::Error::from)
        .context("get all variables")
}

pub async fn set_fastboot_active_slot(dev: &mut NusbFastBoot, slot: &str) -> anyhow::Result<()> {
    dev.set_active(slot)
        .await
        .map_err(anyhow::Error::from)
        .with_context(|| format!("set active slot to {slot}"))
}

pub async fn reboot_device(dev: &mut NusbFastBoot) -> anyhow::Result<()> {
    dev.reboot()
        .await
        .map_err(anyhow::Error::from)
        .context("reboot device")
}

pub async fn reboot_device_bootloader(dev: &mut NusbFastBoot) -> anyhow::Result<()> {
    dev.reboot_bootloader()
        .await
        .map_err(anyhow::Error::from)
        .context("reboot to bootloader")
}

pub async fn power_off_device(dev: &mut NusbFastBoot) -> anyhow::Result<()> {
    dev.power_down()
        .await
        .map_err(anyhow::Error::from)
        .context("power off device")
}

pub async fn send_flashing_unlock(dev: &mut NusbFastBoot) -> anyhow::Result<()> {
    dev.unlock_bootloader()
        .await
        .map_err(anyhow::Error::from)
        .context("unlock bootloader")
}

pub async fn send_flashing_lock(dev: &mut NusbFastBoot) -> anyhow::Result<()> {
    dev.lock_bootloader()
        .await
        .map_err(anyhow::Error::from)
        .context("lock bootloader")
}

pub async fn flash_one_partition(
    dev: &mut NusbFastBoot,
    partition: &str,
    image: &Path,
    on_progress: impl FnMut(FlashProgress),
) -> anyhow::Result<()> {
    let max_download = dev
        .max_download_size()
        .await
        .map_err(anyhow::Error::from)
        .with_context(|| format!("get max download size for {partition}"))?;
    let prepared = prepare_image(image, max_download)
        .with_context(|| format!("prepare image for {partition}"))?;
    flash_prepared_image(dev, partition, &prepared, on_progress)
        .await
        .with_context(|| format!("flash {partition}"))
}

pub async fn erase_one_partition(dev: &mut NusbFastBoot, partition: &str) -> anyhow::Result<()> {
    dev.erase(partition)
        .await
        .map_err(anyhow::Error::from)
        .with_context(|| format!("erase {partition}"))
}

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

pub fn force_fastboot() -> anyhow::Result<()> {
    force_fastboot::run_force_fastboot(&force_fastboot::ForceFastbootOptions::default())
}

#[cfg(test)]
mod tests {
    #[test]
    fn power_off_helper_is_exported() {
        let _ = super::power_off_device;
    }
}
