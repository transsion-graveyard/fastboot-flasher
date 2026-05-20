//! Device-specific execution planning built from an offline preview plan.

use std::collections::HashMap;
use std::path::PathBuf;

use mtk_scatter_parser::{FlashAction, FlashActionExecutionKind, PreviewPlan};
use serde::Serialize;

use crate::device::{resolve_flash_partition_target, resolve_max_download_size_from_vars};
use crate::domain::{filter_actions, resolve_image_path_for_action};

/// Concrete execution route chosen for a preview action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionRoute {
    /// Flash an image directly through fastboot.
    Flash,
    /// Generate a filesystem image from live device information, then flash it.
    FormatData,
    /// Erase a partition only when the device reports it.
    EraseIfPresent,
    /// Do not execute this step because a required prerequisite is missing.
    Blocked,
}

/// One prepared execution step for a connected device.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionStep {
    /// Source preview action.
    pub action: FlashAction,
    /// Device-specific execution route.
    pub route: ExecutionRoute,
    /// Partition name to use on the connected device.
    pub partition_on_device: String,
    /// Resolved image path after applying overrides, when applicable.
    pub image_path: Option<String>,
    /// Whether the partition exists on the connected device, when known.
    pub device_partition_exists: Option<bool>,
    /// Why the step is blocked, when it cannot be executed.
    pub blocking_reason: Option<String>,
}

/// Summary of a prepared execution plan.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExecutionPlanSummary {
    /// Total prepared steps.
    pub step_count: usize,
    /// Number of executable flash steps.
    pub flash_count: usize,
    /// Number of executable wipe/format steps.
    pub wipe_count: usize,
    /// Number of blocked steps.
    pub blocked_count: usize,
    /// Total bytes represented by all steps.
    pub total_bytes: u64,
}

/// Device-specific execution plan built from a preview plan and fastboot vars.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionPlan {
    /// Source offline preview plan.
    pub preview: PreviewPlan,
    /// Prepared steps in execution order.
    pub steps: Vec<ExecutionStep>,
    /// Fastboot variables captured during preparation.
    pub device_variables: HashMap<String, String>,
    /// Parsed device-reported max download size.
    pub max_download_size: u32,
    /// Summary counts for the prepared steps.
    pub summary: ExecutionPlanSummary,
    /// Non-fatal preparation warnings.
    pub warnings: Vec<String>,
    /// Fatal preparation blockers.
    pub errors: Vec<String>,
}

/// Prepare a device-specific execution plan from an offline preview plan.
pub fn prepare_scatter_execution(
    preview: &PreviewPlan,
    partitions: &[String],
    image_overrides: &HashMap<String, String>,
    device_vars: &HashMap<String, String>,
) -> Result<ExecutionPlan, String> {
    let selected_actions = filter_actions(preview, partitions);
    let max_download_size =
        resolve_max_download_size_from_vars(device_vars).map_err(|error| error.to_string())?;
    let mut warnings = preview.warnings.clone();
    let mut errors = preview.errors.clone();
    let mut steps = Vec::with_capacity(selected_actions.len());

    for action in selected_actions {
        let partition_on_device = resolve_flash_partition_target(&action.partition, device_vars);
        let device_partition_exists =
            Some(device_vars.contains_key(&format!("partition-size:{partition_on_device}")));
        let (route, image_path, blocking_reason) =
            route_action(action, image_overrides, &partition_on_device);

        if let Some(reason) = &blocking_reason {
            if action_is_skip_eligible(action) {
                warnings.push(reason.clone());
            } else {
                errors.push(reason.clone());
            }
        }

        steps.push(ExecutionStep {
            action: action.clone(),
            route,
            partition_on_device,
            image_path: image_path.map(|path| path.to_string_lossy().into_owned()),
            device_partition_exists,
            blocking_reason,
        });
    }

    let summary = ExecutionPlanSummary {
        step_count: steps.len(),
        flash_count: steps
            .iter()
            .filter(|step| step.route == ExecutionRoute::Flash)
            .count(),
        wipe_count: steps
            .iter()
            .filter(|step| {
                matches!(
                    step.route,
                    ExecutionRoute::FormatData | ExecutionRoute::EraseIfPresent
                )
            })
            .count(),
        blocked_count: steps
            .iter()
            .filter(|step| step.route == ExecutionRoute::Blocked)
            .count(),
        total_bytes: steps
            .iter()
            .map(|step| u64::try_from(step.action.size).unwrap_or(0))
            .sum(),
    };

    Ok(ExecutionPlan {
        preview: preview.clone(),
        steps,
        device_variables: device_vars.clone(),
        max_download_size,
        summary,
        warnings,
        errors,
    })
}

fn route_action(
    action: &FlashAction,
    image_overrides: &HashMap<String, String>,
    partition_on_device: &str,
) -> (ExecutionRoute, Option<PathBuf>, Option<String>) {
    match action.execution_kind {
        FlashActionExecutionKind::Flash => {
            match resolve_image_path_for_action(action, image_overrides) {
                Ok(path) => (ExecutionRoute::Flash, Some(path), None),
                Err(error) if action_is_skip_eligible(action) => (
                    ExecutionRoute::Flash,
                    None,
                    Some(format!(
                        "missing image path for optional partition {} (target {}): {error}",
                        action.partition, partition_on_device
                    )),
                ),
                Err(error) => (
                    ExecutionRoute::Blocked,
                    None,
                    Some(format!(
                        "missing image path for required partition {} (target {}): {error}",
                        action.partition, partition_on_device
                    )),
                ),
            }
        }
        FlashActionExecutionKind::FormatData => (ExecutionRoute::FormatData, None, None),
        FlashActionExecutionKind::EraseIfPresent => (ExecutionRoute::EraseIfPresent, None, None),
    }
}

fn action_is_skip_eligible(action: &FlashAction) -> bool {
    !matches!(
        action.safety_class.as_str(),
        "bootloader_critical" | "boot_critical" | "android_system"
    )
}
