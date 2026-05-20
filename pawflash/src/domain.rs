#![allow(missing_docs)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::cli::{FlashMode, SlotArg};

use mtk_scatter_parser::{FlashAction, FlashPlan};

/// Message shown when the device reports a power-off command is unsupported.
pub const POWER_OFF_UNSUPPORTED_MESSAGE: &str =
    "Power off is not supported by this device in the current fastboot mode.";

/// Windows-specific driver hint used when device probing fails.
pub const WINDOWS_FASTBOOTD_DRIVER_HINT: &str =
    "On Windows, install the Google USB Driver, then reconnect.";

/// A device snapshot shown to the UI or CLI.
#[derive(Clone, Serialize)]
pub struct DeviceInfo {
    pub serial: String,
    pub product: String,
    pub slot: String,
    pub secure: String,
    pub unlocked: String,
    pub version: String,
    pub mode: String,
    pub all_vars: HashMap<String, String>,
}

/// A plan partition DTO used by the GUI.
#[derive(Clone, Serialize)]
pub struct PartitionDto {
    pub index: usize,
    pub action: String,
    pub partition: String,
    pub size_human: String,
    pub size_bytes: u64,
    pub safety_class: String,
    pub image_type: Option<String>,
    pub source: String,
    pub image_path: Option<String>,
    pub image_name: Option<String>,
    pub selected: bool,
}

/// A flash-plan DTO used by the GUI.
#[derive(Clone, Serialize)]
pub struct FlashPlanDto {
    pub mode: String,
    pub storage: String,
    pub slot_policy: String,
    pub chipset: Option<String>,
    pub summary: FlashSummaryDto,
    pub partitions: Vec<PartitionDto>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

/// The response returned from parsing a scatter file in the GUI.
#[derive(Clone, Serialize)]
pub struct ParseScatterResponseDto {
    pub plan_id: u64,
    pub plan: FlashPlanDto,
}

/// The response returned when the GUI starts a force-fastboot session.
#[derive(Clone, Serialize)]
pub struct ForceFastbootStartDto {
    pub session_id: u64,
}

/// A summary of flash/wipe progress.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FlashSummaryDto {
    pub flash_count: usize,
    pub wipe_count: usize,
    pub skipped_count: usize,
    pub total_bytes: u64,
}

/// Flash progress events shared by the GUI and CLI adapters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", content = "data")]
pub enum FlashEvent {
    WaitingForDevice,
    DeviceCheckDiagnostic {
        stage: String,
        level: String,
        message: String,
    },
    GsiStatus {
        status: String,
    },
    Rebooting {
        target: String,
    },
    PlanBuilt {
        actions: usize,
        total_bytes: u64,
    },
    PreparingImage {
        partition: String,
    },
    Flashing {
        partition: String,
        bytes: u64,
        total: u64,
        speed_bps: u64,
    },
    Simulating {
        partition: String,
        action: String,
        bytes: u64,
        total: u64,
        speed_bps: u64,
    },
    PartitionComplete {
        partition: String,
    },
    PartitionSkipped {
        partition: String,
        reason: String,
    },
    PartitionFailed {
        partition: String,
        error: String,
    },
    Erasing {
        partition: String,
    },
    EraseComplete {
        partition: String,
    },
    Overall {
        bytes: u64,
        total: u64,
    },
    Complete {
        summary: FlashSummaryDto,
    },
    Cancelled {
        message: String,
    },
    Error {
        message: String,
    },
}

/// Force-fastboot progress events shared by the GUI and CLI adapters.
#[derive(Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum ForceFastbootEvent {
    Started { session_id: u64 },
    WaitingForPreloader { session_id: u64 },
    Complete { session_id: u64 },
    Cancelled { session_id: u64 },
    Error { session_id: u64, message: String },
}

/// Cancellation token for long-running flash operations.
#[derive(Clone, Default)]
pub struct FlashRunControl {
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
    ReuseCached,
    Fresh,
}

/// Why fastboot probing failed in the GUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FastbootProbeFailure {
    NoFastbootInterface,
    OpenFailed(String),
    ReadVariablesFailed(String),
}

/// Parsed scatter-plan request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPlanRequest {
    pub mode: FlashMode,
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
        "firmware_upgrade" => Ok(FlashMode::FirmwareUpgrade),
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

/// Normalize the user-facing power-off error string.
pub fn normalize_power_off_error(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("unknown command")
        || lower.contains("unsupported command")
        || lower.contains("not supported")
        || lower.contains("not support")
    {
        return POWER_OFF_UNSUPPORTED_MESSAGE.to_string();
    }
    message.to_string()
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
