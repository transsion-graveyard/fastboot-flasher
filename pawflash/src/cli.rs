//! Shared CLI-facing value types and text rendering helpers.

use mtk_scatter_parser::FlashPlan;
use serde::{Deserialize, Serialize};

/// Flash mode for scatter-based operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlashMode {
    /// Only print what would be done; do not modify the device.
    DryRun,
    /// Perform a firmware upgrade (reflash all partitions from a scatter).
    FirmwareUpgrade,
    /// Wipe userdata and reflash all partitions from a scatter.
    CleanFlash,
    /// Let the user choose which partitions to flash.
    Selective,
}

/// Slot selection argument for partitioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlotArg {
    /// Slot A.
    A,
    /// Slot B.
    B,
    /// The currently active slot.
    Active,
    /// The currently inactive slot.
    Inactive,
    /// Both slots.
    All,
}

/// Reboot target for CLI flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RebootTargetArg {
    /// Reboot to Android/system.
    System,
    /// Reboot to bootloader fastboot.
    Bootloader,
    /// Reboot to userspace fastbootd.
    Fastboot,
    /// Reboot to recovery.
    Recovery,
}

/// Render a human-readable preview of a scatter flash plan.
pub fn scatter_plan_preview_lines(plan: &FlashPlan) -> Vec<String> {
    let total_bytes = plan
        .actions
        .iter()
        .map(|action| u64::try_from(action.size).unwrap_or(0))
        .sum::<u64>();

    let mut lines = vec![
        "scatter plan preview".to_string(),
        format!(
            "mode={} storage={} slot-policy={} layouts={}",
            plan.mode,
            plan.storage_selection,
            plan.slot_policy_effective,
            plan.selected_layouts.join(", ")
        ),
        format!(
            "actions={} flash={} wipe={} skipped={} warnings={} errors={} total={} bytes",
            plan.actions.len(),
            plan.summary.flash_count,
            plan.summary.wipe_count,
            plan.summary.skipped_count,
            plan.summary.warning_count,
            plan.summary.error_count,
            total_bytes
        ),
    ];

    for (index, action) in plan.actions.iter().enumerate() {
        lines.push(format!(
            "{:>2}. {} {} {} [{}] - {}",
            index + 1,
            action.action,
            action.partition,
            action.size_human,
            action.safety_class,
            action.reason
        ));
    }

    if !plan.skipped.is_empty() {
        lines.push(format!("skipped partitions: {}", plan.skipped.len()));
        for skipped in &plan.skipped {
            lines.push(format!(
                "  - {} [{}] - {}",
                skipped.partition, skipped.safety_class, skipped.reason
            ));
        }
    }

    if !plan.warnings.is_empty() {
        lines.push("warnings:".to_string());
        lines.extend(plan.warnings.iter().map(|warning| format!("  - {warning}")));
    }

    if !plan.errors.is_empty() {
        lines.push("errors:".to_string());
        lines.extend(plan.errors.iter().map(|error| format!("  - {error}")));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::scatter_plan_preview_lines;
    use mtk_scatter_parser::{FlashAction, FlashPlan, FlashPlanSummary, SkippedPartition};
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    fn sample_plan() -> FlashPlan {
        FlashPlan {
            mode: "firmware-upgrade".to_string(),
            storage_selection: "auto".to_string(),
            selected_layouts: vec!["UFS".to_string()],
            slot_policy_requested: "auto".to_string(),
            slot_policy_effective: "both".to_string(),
            firmware_dir: Some("/tmp/fw".to_string()),
            package_root: Some("/tmp".to_string()),
            options: json!({}),
            summary: FlashPlanSummary {
                flash_count: 1,
                wipe_count: 0,
                skipped_count: 1,
                missing_image_count: 0,
                oversized_image_count: 0,
                action_warning_count: 0,
                incomplete_slot_base_count: 0,
                warning_count: 1,
                error_count: 1,
            },
            actions: vec![FlashAction {
                action: "flash".to_string(),
                execution_kind: mtk_scatter_parser::FlashActionExecutionKind::Flash,
                partition: "vbmeta_a".to_string(),
                base_name: "vbmeta".to_string(),
                slot: Some("a".to_string()),
                layout: "UFS".to_string(),
                region: "UFS_LU2".to_string(),
                start: 0,
                start_hex: "0x0".to_string(),
                size: 8_388_608,
                size_hex: "0x800000".to_string(),
                size_human: "8.00 MiB".to_string(),
                image: Some(Value::Null),
                image_type: Some("SV5_BL_BIN".to_string()),
                safety_class: "boot_critical".to_string(),
                reason: "allowed by firmware-upgrade".to_string(),
                warnings: vec![],
            }],
            skipped: vec![SkippedPartition {
                partition: "metadata".to_string(),
                layout: "UFS".to_string(),
                region: "UFS_LU0".to_string(),
                reason: "not selected".to_string(),
                safety_class: "wipe_only".to_string(),
                file_name: None,
            }],
            incomplete_slots: BTreeMap::new(),
            warnings: vec!["missing optional image".to_string()],
            errors: vec!["vbmeta_b missing".to_string()],
        }
    }

    #[test]
    fn scatter_plan_preview_lines_include_summary_action_and_diagnostics() {
        let plan = sample_plan();

        let lines = scatter_plan_preview_lines(&plan);

        assert!(lines
            .iter()
            .any(|line| line.contains("scatter plan preview")));
        assert!(lines
            .iter()
            .any(|line| line.contains("mode=firmware-upgrade")));
        assert!(lines
            .iter()
            .any(|line| line.contains("actions=1 flash=1 wipe=0 skipped=1")));
        assert!(lines.iter().any(|line| line.contains("vbmeta_a")));
        assert!(lines
            .iter()
            .any(|line| line.contains("skipped partitions: 1")));
        assert!(lines
            .iter()
            .any(|line| line.contains("missing optional image")));
        assert!(lines.iter().any(|line| line.contains("vbmeta_b missing")));
    }
}
