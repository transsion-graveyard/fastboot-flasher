//! Fastboot device operations: variables, slots, reboot, lock/unlock.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use fastboot_rs::{parse_max_download_size, FastbootDevice};

fn with_device_context<T>(
    result: Result<T, fastboot_rs::FastbootError>,
    context: impl FnOnce() -> String,
) -> anyhow::Result<T> {
    result.map_err(anyhow::Error::from).with_context(context)
}

/// Read a single fastboot variable from the device.
pub async fn read_variable(dev: &mut FastbootDevice, var: &str) -> anyhow::Result<String> {
    with_device_context(dev.get_var(var).await, || format!("get variable {var}"))
}

/// Read all fastboot variables from the device.
pub async fn read_all_variables(
    dev: &mut FastbootDevice,
) -> anyhow::Result<HashMap<String, String>> {
    with_device_context(dev.get_all_vars().await, || "get all variables".to_string())
}

/// Read the `max-download-size` variable from a variables map and parse it.
///
/// # Errors
///
/// Returns an error if the variable is missing, cannot be parsed, or the
/// device reports a zero-sized download limit.
///
/// # Examples
///
/// ```
/// use std::collections::HashMap;
/// use pawflash::resolve_max_download_size_from_vars;
///
/// let vars = HashMap::from([("max-download-size".to_string(), "0x4000000".to_string())]);
/// let max_download = resolve_max_download_size_from_vars(&vars).unwrap();
/// assert_eq!(max_download, 0x4000000);
/// ```
pub fn resolve_max_download_size_from_vars(vars: &HashMap<String, String>) -> anyhow::Result<u32> {
    let Some(raw) = vars.get("max-download-size") else {
        anyhow::bail!("missing fastboot variable max-download-size");
    };
    let max_download =
        parse_max_download_size(raw).with_context(|| format!("parse max-download-size `{raw}`"))?;
    if max_download == 0 {
        anyhow::bail!("device reported max-download-size=0");
    }
    Ok(max_download)
}

/// Resolve a flash target to the partition name the device most likely exposes.
///
/// This keeps the exact scatter/device name when present, but falls back to the
/// unsuffixed base partition for slot-suffixed names such as `vbmeta_a` when the
/// device only advertises `vbmeta`.
pub fn resolve_flash_partition_target(partition: &str, vars: &HashMap<String, String>) -> String {
    let exact_key = format!("partition-size:{partition}");
    if vars.contains_key(&exact_key) {
        return partition.to_string();
    }

    if let Some(base) = partition
        .strip_suffix("_a")
        .or_else(|| partition.strip_suffix("_b"))
    {
        let base_key = format!("partition-size:{base}");
        if vars.contains_key(&base_key) {
            return base.to_string();
        }
    }

    partition.to_string()
}

/// Set the active boot slot on the device.
pub async fn set_fastboot_active_slot(dev: &mut FastbootDevice, slot: &str) -> anyhow::Result<()> {
    with_device_context(dev.set_active(slot).await, || {
        format!("set active slot to {slot}")
    })
}

/// Reboot the device into the normal OS.
pub async fn reboot_device(dev: &mut FastbootDevice) -> anyhow::Result<()> {
    with_device_context(dev.reboot().await, || "reboot device".to_string())
}

/// Reboot the device into the bootloader.
pub async fn reboot_device_bootloader(dev: &mut FastbootDevice) -> anyhow::Result<()> {
    with_device_context(dev.reboot_bootloader().await, || {
        "reboot to bootloader".to_string()
    })
}

/// Reboot the device into fastbootd (userspace fastboot).
pub async fn reboot_device_fastboot(dev: &mut FastbootDevice) -> anyhow::Result<()> {
    with_device_context(dev.reboot_fastboot().await, || {
        "reboot to fastboot".to_string()
    })
}

/// Power off the device.
pub async fn power_off_device(dev: &mut FastbootDevice) -> anyhow::Result<()> {
    with_device_context(dev.power_down().await, || "power off device".to_string())
}

/// Send the `flashing unlock` command to unlock the bootloader.
pub async fn send_flashing_unlock(dev: &mut FastbootDevice) -> anyhow::Result<()> {
    with_device_context(dev.unlock_bootloader().await, || {
        "unlock bootloader".to_string()
    })
}

/// Send the `flashing lock` command to lock the bootloader.
pub async fn send_flashing_lock(dev: &mut FastbootDevice) -> anyhow::Result<()> {
    with_device_context(dev.lock_bootloader().await, || {
        "lock bootloader".to_string()
    })
}

/// Build a flash plan by parsing a scatter file with the given mode, slot,
/// preloader, and partition filters.
///
/// # Errors
///
/// Returns an error when the scatter file cannot be parsed or the derived
/// flash plan fails validation.
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
    fn device_error_wrapper_preserves_context() {
        let error = fastboot_rs::FastbootError::Download("boom".to_string());

        let wrapped: anyhow::Result<()> =
            super::with_device_context(Err(error), || "erase userdata".to_string());
        let wrapped = wrapped.unwrap_err();

        assert!(wrapped.to_string().contains("erase userdata"));
        assert!(wrapped
            .chain()
            .any(|cause| cause.to_string().contains("boom")));
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

    #[test]
    fn resolve_flash_partition_target_keeps_exact_match() {
        let vars = HashMap::from([
            ("partition-size:vbmeta_a".to_string(), "0x1000".to_string()),
            ("partition-size:vbmeta".to_string(), "0x1000".to_string()),
        ]);

        let resolved = super::resolve_flash_partition_target("vbmeta_a", &vars);

        assert_eq!(resolved, "vbmeta_a");
    }

    #[test]
    fn resolve_flash_partition_target_falls_back_to_unsuffixed_base() {
        let vars = HashMap::from([("partition-size:vbmeta".to_string(), "0x1000".to_string())]);

        let resolved = super::resolve_flash_partition_target("vbmeta_a", &vars);

        assert_eq!(resolved, "vbmeta");
    }
}
