#![cfg_attr(not(windows), deny(unsafe_code))]
#![deny(missing_docs)]

//! Shared business logic for `pawflash` CLI and `pawflash-gui`.
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
pub use fastboot_rs::{FastbootDevice, FastbootError, FastbootExecutionError, FlashProgress};

// Re-export from force-fastboot
pub use force_fastboot::{run_force_fastboot, ForceFastbootError, ForceFastbootOptions};

// Re-export from mtk-scatter-parser
pub use mtk_scatter_parser::{FlashAction, FlashPlan, FlashPlanOptions, Mode, SlotPolicy};

// Re-export helpers needed by submodules
pub use connect::connect_fastboot;
pub use device::{
    build_flash_plan, power_off_device, read_all_variables, read_variable, reboot_device,
    reboot_device_bootloader, reboot_device_fastboot, resolve_max_download_size_from_vars,
    send_flashing_lock, send_flashing_unlock, set_fastboot_active_slot,
};
pub use flash::{erase_one_partition, flash_one_partition};

/// Convenience wrapper that runs the default force-fastboot flow.
pub fn force_fastboot() -> Result<(), ForceFastbootError> {
    run_force_fastboot(&ForceFastbootOptions::default())
}

#[cfg(test)]
mod tests {
    #[test]
    fn power_off_helper_is_exported() {
        let _ = super::device::power_off_device;
    }

    #[test]
    fn reboot_fastboot_helper_is_exported() {
        let _ = super::device::reboot_device_fastboot;
    }
}
