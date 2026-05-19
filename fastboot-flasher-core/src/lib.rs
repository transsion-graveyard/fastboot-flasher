#![cfg_attr(not(windows), deny(unsafe_code))]
#![deny(missing_docs)]

//! Shared business logic for `fastboot-flasher` CLI and `fastboot-flasher-gui`.
//!
//! This crate contains all device operations, flash flows, GSI handling,
//! and format/wipe logic. It has **zero** terminal or UI dependencies so
//! both the CLI and GUI binaries can depend on it cleanly.

pub mod cli;
pub mod connect;
pub mod device;
pub mod device_info;
pub mod flash;
pub mod format;
pub mod gsi;
pub mod manual;
pub mod plan;
pub mod progress;

// Re-export types from protocol crates
pub use fastboot_rs::{
    FastbootDevice, FastbootError, FastbootExecutionError, FlashProgress,
};

// Re-export from force-fastboot
pub use force_fastboot::{run_force_fastboot, ForceFastbootOptions, ForceFastbootError};

// Re-export from mtk-scatter-parser
pub use mtk_scatter_parser::{FlashPlan, FlashAction, FlashPlanOptions, Mode, SlotPolicy};

// Re-export helpers needed by submodules
pub use connect::connect_fastboot;
pub use device::{
    build_flash_plan, power_off_device, reboot_device, reboot_device_bootloader,
    reboot_device_fastboot, read_all_variables, read_variable, resolve_max_download_size_from_vars,
    send_flashing_lock, send_flashing_unlock, set_fastboot_active_slot,
};
pub use flash::{erase_one_partition, flash_one_partition};

/// Convenience wrapper that runs the default force-fastboot flow.
pub fn force_fastboot() -> Result<(), ForceFastbootError> {
    run_force_fastboot(&ForceFastbootOptions::default())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    #[test]
    fn power_off_helper_is_exported() {
        let _ = super::device::power_off_device;
    }

    #[test]
    fn reboot_fastboot_helper_is_exported() {
        let _ = super::device::reboot_device_fastboot;
    }

    #[test]
    fn resolve_max_download_size_from_vars_accepts_hex_values() {
        let vars = HashMap::from([("max-download-size".to_string(), "0x4000000".to_string())]);

        let max_download = super::device::resolve_max_download_size_from_vars(&vars).unwrap();

        assert_eq!(max_download, 0x4000000);
    }

    #[test]
    fn resolve_max_download_size_from_vars_rejects_zero() {
        let vars = HashMap::from([("max-download-size".to_string(), "0".to_string())]);

        let error = super::device::resolve_max_download_size_from_vars(&vars).unwrap_err();

        assert!(error.to_string().contains("max-download-size=0"));
    }

    #[test]
    fn resolve_max_download_size_from_vars_requires_variable() {
        let error = super::device::resolve_max_download_size_from_vars(&HashMap::new()).unwrap_err();

        assert!(error.to_string().contains("max-download-size"));
    }
}
