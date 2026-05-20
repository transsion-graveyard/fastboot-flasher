#![cfg_attr(not(windows), deny(unsafe_code))]
#![deny(missing_docs)]

//! Shared business logic for `pawflash` CLI and `pawflash-gui`.
//!
//! This crate contains all device operations, flash flows, GSI handling,
//! and format/wipe logic. It has **zero** terminal or UI dependencies so
//! both the CLI and GUI binaries can depend on it cleanly.

/// Command-line argument types and shared parsing helpers.
pub mod cli;
/// Device discovery and connection helpers.
pub mod connect;
/// Fastboot device operations and plan construction.
pub mod device;
/// Human-readable device information rendering.
pub mod device_info;
/// Shared domain types and helpers for adapters.
pub mod domain;
/// Partition flashing and erase helpers.
pub mod flash;
/// Formatting and wipe orchestration helpers.
pub mod format;
/// GSI-specific planning and flashing workflows.
pub mod gsi;
/// Manual flashing helpers and vbmeta utilities.
pub mod manual;
/// Flash-plan helpers and data shaping.
pub mod plan;
/// Shared progress formatting and simulation helpers.
pub mod progress;
/// Shared workflow helpers used by CLI and GUI adapters.
pub mod workflow;

// Re-export types from protocol crates
pub use domain::{
    build_device_check_diagnostic, default_partition_selected, describe_fastboot_probe_failure,
    display_safety_class, filter_actions, normalize_slot, normalize_storage_label,
    parse_flash_mode, parse_plan_request, parse_slot, plan_requires_connected_device, plan_to_dto,
    resolve_image_path_for_action, total_bytes_for_actions, update_overall_progress, DeviceInfo,
    DeviceSessionPolicy, FastbootProbeFailure, FlashEvent, FlashOperation, FlashPlanDto,
    FlashRunControl, FlashSummaryDto, ForceFastbootEvent, ForceFastbootStartDto,
    ParseScatterResponseDto, ParsedPlanRequest, PartitionDto, WINDOWS_FASTBOOTD_DRIVER_HINT,
};
pub use fastboot_rs::{FastbootDevice, FastbootError, FastbootExecutionError, FlashProgress};

// Re-export from force-fastboot
pub use force_fastboot::{run_force_fastboot, ForceFastbootError, ForceFastbootOptions};

// Re-export from mtk-scatter-parser
pub use mtk_scatter_parser::{
    FlashAction, FlashActionExecutionKind, FlashPlan, FlashPlanOptions, Mode, SlotPolicy,
};

// Re-export helpers needed by submodules
pub use connect::connect_fastboot;
pub use device::{
    build_flash_plan, read_all_variables, read_variable, reboot_device, reboot_device_bootloader,
    reboot_device_fastboot, resolve_max_download_size_from_vars, send_flashing_lock,
    send_flashing_unlock, set_fastboot_active_slot,
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
    fn reboot_fastboot_helper_is_exported() {
        let _ = super::device::reboot_device_fastboot;
    }
}
