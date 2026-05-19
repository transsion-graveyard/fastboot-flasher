//! Fastboot device operations: variables, slots, reboot, lock/unlock.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use fastboot_rs::{
    parse_max_download_size, FastbootDevice,
};

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

/// Build a flash plan by parsing a scatter file with the given mode, slot,
/// preloader, and partition filters.
pub fn build_flash_plan(
    scatter_path: &Path,
    mode: crate::cli::FlashMode,
    slot: Option<crate::cli::SlotArg>,
    include_preloader: bool,
    parts: &[String],
    check_images: bool,
) -> anyhow::Result<mtk_scatter_parser::FlashPlan> {
    crate::plan::build_plan_checked(
        scatter_path,
        mode,
        slot,
        include_preloader,
        parts,
        check_images,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

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