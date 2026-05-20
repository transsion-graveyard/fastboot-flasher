use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::cli::{FlashMode, SlotArg};

use mtk_scatter_parser::{FlashAction, FlashPlan};

/// Windows-specific driver hint used when device probing fails.
pub const WINDOWS_FASTBOOTD_DRIVER_HINT: &str =
    "On Windows, install the Google USB Driver, then reconnect.";

/// A device snapshot shown to the UI or CLI.
#[derive(Clone, Serialize)]
pub struct DeviceInfo {
    /// Fastboot serial number reported by the device.
    pub serial: String,
    /// Product identifier reported by the device.
    pub product: String,
    /// Active slot label reported by the device.
    pub slot: String,
    /// Whether secure boot is enabled.
    pub secure: String,
    /// Whether the bootloader is unlocked.
    pub unlocked: String,
    /// Fastboot protocol or bootloader version string.
    pub version: String,
    /// Current fastboot mode or transport mode.
    pub mode: String,
    /// Raw fastboot variables collected from the device.
    pub all_vars: HashMap<String, String>,
}

/// A plan partition DTO used by the GUI.
#[derive(Clone, Serialize)]
pub struct PartitionDto {
    /// Stable row index in the rendered plan.
    pub index: usize,
    /// Action type such as `flash` or `wipe`.
    pub action: String,
    /// Target partition name.
    pub partition: String,
    /// Human-readable size string for display.
    pub size_human: String,
    /// Raw byte count for the action.
    pub size_bytes: u64,
    /// Normalized safety-class label.
    pub safety_class: String,
    /// Optional image type reported by the scatter parser.
    pub image_type: Option<String>,
    /// Why the action was included in the plan.
    pub source: String,
    /// Resolved absolute or relative image path when available.
    pub image_path: Option<String>,
    /// Basename of the image selected for flashing.
    pub image_name: Option<String>,
    /// Whether this action should be shown as a selectable row in the GUI.
    pub user_visible: bool,
    /// Whether the GUI should preselect this action.
    pub selected: bool,
}

/// A flash-plan DTO used by the GUI.
#[derive(Clone, Serialize)]
pub struct FlashPlanDto {
    /// Requested flash mode.
    pub mode: String,
    /// Normalized storage label for display.
    pub storage: String,
    /// Effective slot policy name.
    pub slot_policy: String,
    /// Optional chipset value derived from the scatter input.
    pub chipset: Option<String>,
    /// Summary counts and totals for the plan.
    pub summary: FlashSummaryDto,
    /// Ordered partition actions included in the plan.
    pub partitions: Vec<PartitionDto>,
    /// Non-fatal warnings surfaced while building the plan.
    pub warnings: Vec<String>,
    /// Fatal validation errors surfaced while building the plan.
    pub errors: Vec<String>,
}

/// The response returned from parsing a scatter file in the GUI.
#[derive(Clone, Serialize)]
pub struct ParseScatterResponseDto {
    /// Opaque identifier for the cached parsed plan.
    pub plan_id: u64,
    /// Parsed plan data to render in the UI.
    pub plan: FlashPlanDto,
}

/// The response returned when the GUI starts a force-fastboot session.
#[derive(Clone, Serialize)]
pub struct ForceFastbootStartDto {
    /// Opaque identifier for the background session.
    pub session_id: u64,
}

/// A summary of flash/wipe progress.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FlashSummaryDto {
    /// Number of successful flash actions completed.
    pub flash_count: usize,
    /// Number of successful wipe actions completed.
    pub wipe_count: usize,
    /// Number of actions skipped after non-fatal failures.
    pub skipped_count: usize,
    /// Total bytes represented by the run or plan.
    pub total_bytes: u64,
}

/// Semantic operation kind for progress events.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FlashOperation {
    /// A normal partition flash.
    Flash,
    /// A data-format operation implemented by flashing a generated blank image.
    FormatData,
    /// An erase operation.
    Erase,
}

/// Flash progress events shared by the GUI and CLI adapters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", content = "data")]
pub enum FlashEvent {
    /// Waiting for a device to appear or reconnect.
    WaitingForDevice,
    /// A diagnostic message emitted while checking device readiness.
    DeviceCheckDiagnostic {
        /// Diagnostic stage name.
        stage: String,
        /// Severity level such as `info`, `warn`, or `error`.
        level: String,
        /// Human-readable diagnostic message.
        message: String,
    },
    /// A coarse-grained GSI workflow status update.
    GsiStatus {
        /// Status message to show to the user.
        status: String,
    },
    /// A reboot command is about to be sent.
    Rebooting {
        /// Reboot target such as `system` or `bootloader`.
        target: String,
    },
    /// A plan has been accepted for execution.
    PlanBuilt {
        /// Number of actions in the run.
        actions: usize,
        /// Total bytes represented by the plan.
        total_bytes: u64,
    },
    /// Image preparation has started for a partition.
    PreparingImage {
        /// Partition being prepared.
        partition: String,
        /// Semantic operation being prepared.
        operation: FlashOperation,
    },
    /// Bytes are currently being flashed to a partition.
    Flashing {
        /// Partition being flashed.
        partition: String,
        /// Semantic operation in progress.
        operation: FlashOperation,
        /// Bytes transferred so far for this partition.
        bytes: u64,
        /// Total bytes expected for this partition.
        total: u64,
        /// Approximate transfer speed in bytes per second.
        speed_bps: u64,
    },
    /// Simulated progress for dry-run execution.
    Simulating {
        /// Partition being simulated.
        partition: String,
        /// Semantic operation being simulated.
        operation: FlashOperation,
        /// Bytes progressed so far.
        bytes: u64,
        /// Total bytes represented by the simulated action.
        total: u64,
        /// Approximate synthetic transfer speed in bytes per second.
        speed_bps: u64,
    },
    /// A partition flash action finished successfully.
    PartitionComplete {
        /// Partition that completed.
        partition: String,
        /// Semantic operation that completed.
        operation: FlashOperation,
    },
    /// A partition action was skipped after a non-fatal issue.
    PartitionSkipped {
        /// Partition that was skipped.
        partition: String,
        /// Semantic operation that was skipped.
        operation: FlashOperation,
        /// User-facing reason for the skip.
        reason: String,
    },
    /// A partition action failed fatally.
    PartitionFailed {
        /// Partition that failed.
        partition: String,
        /// Semantic operation that failed.
        operation: FlashOperation,
        /// User-facing error message.
        error: String,
    },
    /// A partition erase action has started.
    Erasing {
        /// Partition being erased.
        partition: String,
    },
    /// A partition erase action finished successfully.
    EraseComplete {
        /// Partition that was erased.
        partition: String,
    },
    /// Aggregate progress across the full run.
    Overall {
        /// Bytes completed so far.
        bytes: u64,
        /// Total bytes represented by the run.
        total: u64,
    },
    /// The run completed successfully.
    Complete {
        /// Final summary for the run.
        summary: FlashSummaryDto,
    },
    /// The run was cancelled by the user.
    Cancelled {
        /// User-facing cancellation message.
        message: String,
    },
    /// The run failed before completion.
    Error {
        /// User-facing error message.
        message: String,
    },
}

/// Force-fastboot progress events shared by the GUI and CLI adapters.
#[derive(Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum ForceFastbootEvent {
    /// A force-fastboot session has started.
    Started {
        /// Opaque identifier for the force-fastboot session.
        session_id: u64,
    },
    /// The session is waiting for a preloader connection.
    WaitingForPreloader {
        /// Opaque identifier for the force-fastboot session.
        session_id: u64,
    },
    /// The force-fastboot session completed successfully.
    Complete {
        /// Opaque identifier for the force-fastboot session.
        session_id: u64,
    },
    /// The force-fastboot session was cancelled.
    Cancelled {
        /// Opaque identifier for the force-fastboot session.
        session_id: u64,
    },
    /// The force-fastboot session failed.
    Error {
        /// Opaque identifier for the force-fastboot session.
        session_id: u64,
        /// User-facing failure message.
        message: String,
    },
}

/// Cancellation token for long-running flash operations.
#[derive(Clone, Default)]
pub struct FlashRunControl {
    /// Shared cancellation flag checked by long-running tasks.
    pub cancel_requested: Arc<AtomicBool>,
}

impl FlashRunControl {
    /// Reset cancellation before a new run.
    pub fn begin(&self) {
        self.cancel_requested.store(false, Ordering::SeqCst);
    }

    /// Ask the current run to stop.
    pub fn request_cancel(&self) {
        self.cancel_requested.store(true, Ordering::SeqCst);
    }

    /// Return an error if the current run has been cancelled.
    pub fn ensure_not_cancelled(&self) -> Result<(), String> {
        if self.cancel_requested.load(Ordering::SeqCst) {
            Err("cancelled by user".to_string())
        } else {
            Ok(())
        }
    }
}

/// Policy for reusing a cached device session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceSessionPolicy {
    /// Reuse the cached device connection when present.
    ReuseCached,
    /// Create a fresh device connection.
    Fresh,
}

/// Why fastboot probing failed in the GUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FastbootProbeFailure {
    /// No compatible fastboot interface was detected.
    NoFastbootInterface,
    /// The device was found but opening it failed.
    OpenFailed(String),
    /// The device opened but reading fastboot variables failed.
    ReadVariablesFailed(String),
}

/// Parsed scatter-plan request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPlanRequest {
    /// Requested flash mode.
    pub mode: FlashMode,
    /// Optional slot constraint.
    pub slot: Option<SlotArg>,
}

/// Build the GUI diagnostic event for device probing.
pub fn build_device_check_diagnostic(
    stage: &str,
    level: &str,
    message: impl Into<String>,
) -> FlashEvent {
    FlashEvent::DeviceCheckDiagnostic {
        stage: stage.to_string(),
        level: level.to_string(),
        message: message.into(),
    }
}

/// Turn a probe failure into a user-facing message.
pub fn describe_fastboot_probe_failure(failure: &FastbootProbeFailure) -> String {
    match failure {
        FastbootProbeFailure::NoFastbootInterface => format!(
            "No fastboot device detected. {WINDOWS_FASTBOOTD_DRIVER_HINT}"
        ),
        FastbootProbeFailure::OpenFailed(error) => format!(
            "Fastboot device detected but could not be opened: {error}. {WINDOWS_FASTBOOTD_DRIVER_HINT}"
        ),
        FastbootProbeFailure::ReadVariablesFailed(error) => format!(
            "Fastboot device opened but did not respond to fastboot variables: {error}. {WINDOWS_FASTBOOTD_DRIVER_HINT}"
        ),
    }
}

/// Parse a GUI plan request from strings.
pub fn parse_plan_request(mode: &str, slot: Option<&str>) -> Result<ParsedPlanRequest, String> {
    Ok(ParsedPlanRequest {
        mode: parse_flash_mode(mode)?,
        slot: parse_slot(slot),
    })
}

/// Parse a flash mode from the GUI string format.
pub fn parse_flash_mode(mode: &str) -> Result<FlashMode, String> {
    match mode {
        "dry_run" => Ok(FlashMode::DryRun),
        "dirty_flash" => Ok(FlashMode::DirtyFlash),
        "clean_flash" => Ok(FlashMode::CleanFlash),
        "selective" => Ok(FlashMode::Selective),
        other => Err(format!("unknown flash mode: {other}")),
    }
}

/// Parse a slot string from the GUI string format.
pub fn parse_slot(slot: Option<&str>) -> Option<SlotArg> {
    match slot {
        Some("a") => Some(SlotArg::A),
        Some("b") => Some(SlotArg::B),
        Some("active") => Some(SlotArg::Active),
        Some("inactive") => Some(SlotArg::Inactive),
        Some("all") => Some(SlotArg::All),
        _ => None,
    }
}

/// Normalize a slot string for comparisons.
pub fn normalize_slot(slot: Option<&String>) -> Option<String> {
    match slot.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if value == "a" || value == "b" => Some(value),
        _ => None,
    }
}

/// Keep only the requested flash actions.
pub fn filter_actions<'a>(plan: &'a FlashPlan, partitions: &[String]) -> Vec<&'a FlashAction> {
    if partitions.is_empty() {
        return plan.actions.iter().collect();
    }

    plan.actions
        .iter()
        .filter(|action| partitions.contains(&action.partition))
        .collect()
}

/// Whether a flash plan requires a connected device.
pub fn plan_requires_connected_device(plan: &FlashPlan) -> bool {
    !matches!(plan.mode.as_str(), "dry_run" | "dry-run")
}

/// Sum the byte sizes for the selected flash actions.
pub fn total_bytes_for_actions(actions: &[&FlashAction]) -> u64 {
    actions
        .iter()
        .map(|action| u64::try_from(action.size).unwrap_or(0))
        .sum()
}

/// Update overall progress from the bytes completed so far.
pub fn update_overall_progress(
    completed_before: u64,
    current_bytes: u64,
    total_bytes: u64,
) -> (u64, u64) {
    (
        completed_before
            .saturating_add(current_bytes)
            .min(total_bytes),
        total_bytes,
    )
}

/// Resolve a flash-image path for a plan action.
pub fn resolve_image_path_for_action(
    action: &FlashAction,
    image_overrides: &HashMap<String, String>,
) -> Result<PathBuf, String> {
    if let Some(path) = image_overrides.get(&action.partition) {
        return Ok(PathBuf::from(path));
    }

    action
        .image_resolved_path()
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing image path for {}", action.partition))
}

/// Display a human-readable storage label.
pub fn normalize_storage_label(storage: &str, selected_layouts: &[String]) -> String {
    let selected = selected_layouts.join(" ").to_uppercase();
    if selected.contains("UFS") {
        return "UFS".to_string();
    }
    if selected.contains("EMMC") || selected.contains("MMC") {
        return "EMMC".to_string();
    }

    let upper = storage.to_uppercase();
    if upper.contains("UFS") {
        "UFS".to_string()
    } else if upper.contains("EMMC") || upper.contains("MMC") {
        "EMMC".to_string()
    } else {
        storage.to_string()
    }
}

/// Display a normalized safety class label.
pub fn display_safety_class(safety_class: &str) -> String {
    match safety_class {
        "firmware" => "firmware",
        "android_system" => "android_system",
        "wipe_only" => "wipe_only",
        "identity_or_calibration" => "identity_or_calibration",
        "dangerous" => "dangerous",
        "bootloader_critical" => "bootloader_critical",
        "boot_critical" => "boot_critical",
        "regional" => "regional",
        "unknown" => "other",
        other => other,
    }
    .to_string()
}

/// Default GUI selection state for a plan action.
pub fn default_partition_selected(action: &FlashAction) -> bool {
    if action.action != "flash" {
        return true;
    }

    matches!(action.image_exists(), Some(true))
}

fn partition_user_visible(plan: &FlashPlan, action: &FlashAction) -> bool {
    if !matches!(plan.mode.as_str(), "clean-flash" | "clean_flash") {
        return true;
    }

    if action.action == "wipe" && matches!(action.partition.as_str(), "metadata" | "cache") {
        return false;
    }

    if action.partition == "userdata" && action.action == "wipe" {
        let has_userdata_flash = plan
            .actions
            .iter()
            .any(|candidate| candidate.partition == "userdata" && candidate.action == "flash");
        if has_userdata_flash {
            return false;
        }
    }

    true
}

/// Convert a flash plan to the GUI DTO.
pub fn plan_to_dto(plan: &FlashPlan, chipset: Option<String>) -> FlashPlanDto {
    let partitions = plan
        .actions
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let image_path = a.image_resolved_path().map(ToOwned::to_owned);
            let image_name = image_path
                .as_deref()
                .and_then(|path| {
                    PathBuf::from(path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .or_else(|| {
                    a.image
                        .as_ref()
                        .and_then(|image| {
                            image.pointer("/file_name").and_then(|value| value.as_str())
                        })
                        .map(ToOwned::to_owned)
                });

            PartitionDto {
                index: i,
                action: a.action.clone(),
                partition: a.partition.clone(),
                size_human: a.size_human.clone(),
                size_bytes: u64::try_from(a.size).unwrap_or(0),
                safety_class: display_safety_class(&a.safety_class),
                image_type: a.image_type.clone(),
                source: a.reason.clone(),
                image_path,
                image_name,
                user_visible: partition_user_visible(plan, a),
                selected: default_partition_selected(a),
            }
        })
        .collect();

    FlashPlanDto {
        mode: plan.mode.clone(),
        storage: normalize_storage_label(&plan.storage_selection, &plan.selected_layouts),
        slot_policy: plan.slot_policy_effective.clone(),
        chipset,
        summary: FlashSummaryDto {
            flash_count: plan.summary.flash_count,
            wipe_count: plan.summary.wipe_count,
            skipped_count: plan.summary.skipped_count,
            total_bytes: plan
                .actions
                .iter()
                .map(|a| u64::try_from(a.size).unwrap_or(0))
                .sum(),
        },
        partitions,
        warnings: plan.warnings.clone(),
        errors: plan.errors.clone(),
    }
}
