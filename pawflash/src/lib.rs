#![cfg_attr(not(windows), deny(unsafe_code))]
#![deny(missing_docs)]
#![allow(missing_docs)]

//! Shared business logic for `pawflash` CLI and `pawflash-gui`.
//!
//! This crate contains all device operations, flash flows, GSI handling,
//! and format/wipe logic. It has **zero** terminal or UI dependencies so
//! both the CLI and GUI binaries can depend on it cleanly.

pub mod cli;
pub mod connect;
pub mod device;
pub mod device_info;
/// Shared domain types and helpers for adapters.
pub mod domain;
pub mod flash;
pub mod format;
pub mod gsi;
pub mod manual;
pub mod plan;
pub mod progress;
/// Shared workflow helpers used by CLI and GUI adapters.
pub mod workflow;

// Re-export types from protocol crates
pub use domain::{
    build_device_check_diagnostic, default_partition_selected, describe_fastboot_probe_failure,
    display_safety_class, filter_actions, normalize_power_off_error, normalize_slot,
    normalize_storage_label, parse_flash_mode, parse_plan_request, parse_slot,
    plan_requires_connected_device, plan_to_dto, resolve_image_path_for_action,
    total_bytes_for_actions, update_overall_progress, DeviceInfo, DeviceSessionPolicy,
    FastbootProbeFailure, FlashEvent, FlashPlanDto, FlashRunControl, FlashSummaryDto,
    ForceFastbootEvent, ForceFastbootStartDto, ParseScatterResponseDto, ParsedPlanRequest,
    PartitionDto, POWER_OFF_UNSUPPORTED_MESSAGE, WINDOWS_FASTBOOTD_DRIVER_HINT,
};
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
pub use flash::{
    erase_one_partition, flash_one_partition, flash_one_partition_with_resize,
    ResizeLogicalPartition,
};

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
