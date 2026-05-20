//! CLI argument parsing and validation using `clap`.

use anyhow::Context;
use inquire::Confirm;
use mtk_scatter_parser::FlashPlan;
use std::path::PathBuf;

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};

/// Flash mode for scatter-based operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
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

/// PawFlash subcommands.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Wait for an MTK preloader device and nudge it into fastboot mode.
    ForceFastboot,
    /// Flash the bundled empty vbmeta image to vbmeta_a and vbmeta_b.
    DisableVbmeta,
    /// Flash a GSI system image using the app's GSI flow.
    Gsi {
        /// Path to the GSI system image.
        image: PathBuf,
    },
    /// Format userdata like recovery's "Format Data".
    Format {
        /// Partition to format (currently only supports `userdata`).
        partition: String,
        /// Fall back to `fastboot erase` if image generation fails.
        #[arg(long)]
        erase_fallback: bool,
    },
    /// Build a scatter plan and optionally execute it.
    #[command(arg_required_else_help = true)]
    Scatter {
        /// Path to the MTK scatter file.
        scatter: PathBuf,
        /// Perform a firmware upgrade (reflash all partitions).
        #[arg(long)]
        firmware_upgrade: bool,
        /// Wipe userdata and reflash all partitions.
        #[arg(long)]
        clean_flash: bool,
        /// Let the user choose which partitions to flash.
        #[arg(long)]
        selective: bool,
        /// Slot selection for A/B partitions.
        #[arg(long, value_enum)]
        slot: Option<SlotArg>,
        /// Include preloader partitions in the flash plan.
        #[arg(long)]
        include_preloader: bool,
        /// Reboot to system immediately after a successful flash.
        #[arg(long)]
        reboot: bool,
    },
    /// Flash one image to one exact fastboot partition.
    Flash {
        /// Partition to flash.
        partition: String,
        /// Image file to flash.
        image: PathBuf,
        /// Slot selection for A/B partitions.
        #[arg(long, value_enum)]
        slot: Option<SlotArg>,
    },
    /// Reboot the current fastboot device.
    Reboot {
        /// Reboot target.
        #[arg(long, value_enum, default_value_t = RebootTargetArg::System)]
        target: RebootTargetArg,
    },
    /// Read a single fastboot variable.
    Getvar {
        /// Variable name to read.
        var: String,
    },
    /// Read all fastboot variables.
    GetvarAll,
    /// Set the active slot.
    SetActive {
        /// Slot to activate.
        #[arg(value_enum)]
        slot: SlotArg,
    },
    /// Send `flashing unlock`.
    UnlockBootloader,
    /// Send `flashing lock`.
    LockBootloader,
    /// Format userdata, then best-effort erase metadata and cache.
    WipeData {
        /// Skip erasing the metadata partition.
        #[arg(long)]
        no_metadata: bool,
        /// Skip erasing the cache partition.
        #[arg(long)]
        no_cache: bool,
        /// Fall back to `fastboot erase userdata` if image generation fails.
        #[arg(long)]
        erase_fallback: bool,
    },
}

/// Top-level CLI arguments.
#[derive(Debug, Parser)]
#[command(about = "Plan and execute safe MTK fastboot flashing flows")]
#[command(group(ArgGroup::new("flash_mode").args([
    "dry_run",
    "firmware_upgrade",
    "clean_flash",
    "selective",
])))]
pub struct Args {
    /// Optional subcommand.
    #[command(subcommand)]
    pub command: Option<Command>,
    /// Try to force the device into fastboot mode before proceeding.
    #[arg(long)]
    pub force_fastboot: bool,
    /// Path to the scatter file for scatter-based flashing.
    #[arg(long)]
    pub flash: Option<PathBuf>,
    /// Dry-run mode (implies `--flash`).
    #[arg(long, global = true)]
    pub dry_run: bool,
    /// Firmware-upgrade mode (implies `--flash`).
    #[arg(long, requires = "flash")]
    pub firmware_upgrade: bool,
    /// Clean-flash mode (implies `--flash`).
    #[arg(long, requires = "flash")]
    pub clean_flash: bool,
    /// Selective-flash mode (implies `--flash`).
    #[arg(long, requires = "flash")]
    pub selective: bool,
    /// Slot selection (implies `--flash`).
    #[arg(long, value_enum, requires = "flash")]
    pub slot: Option<SlotArg>,
    /// Include preloader partitions when flashing from a scatter.
    #[arg(long, requires = "flash")]
    pub include_preloader: bool,
    /// Auto-answer yes to all prompts.
    #[arg(long, global = true)]
    pub yes: bool,
    /// Set the active slot.
    #[arg(long)]
    pub set_active: Option<SlotArg>,
    /// Reboot the device after finishing.
    #[arg(long)]
    pub reboot: bool,
    /// Reboot the device into the bootloader.
    #[arg(long)]
    pub reboot_bootloader: bool,
    /// Read a single fastboot variable.
    #[arg(long)]
    pub getvar: Option<String>,
    /// Read all fastboot variables.
    #[arg(long)]
    pub getvar_all: bool,
}

impl Args {
    /// Determine the [`FlashMode`] from the boolean flags.
    pub fn flash_mode(&self) -> FlashMode {
        if self.firmware_upgrade {
            FlashMode::FirmwareUpgrade
        } else if self.clean_flash {
            FlashMode::CleanFlash
        } else if self.selective {
            FlashMode::Selective
        } else {
            FlashMode::DryRun
        }
    }
}

/// Check if any flash-mode modifier flags are present without `--flash` (or a
/// subcommand) and return an error message if so.
pub fn flash_modifier_without_flash(args: &Args) -> Result<(), String> {
    if args.flash.is_some() || args.command.is_some() {
        return Ok(());
    }
    let modifiers = [
        (args.dry_run, "--dry-run"),
        (args.firmware_upgrade, "--firmware-upgrade"),
        (args.clean_flash, "--clean-flash"),
        (args.selective, "--selective"),
        (args.slot.is_some(), "--slot"),
        (args.include_preloader, "--include-preloader"),
    ];
    let used = modifiers
        .iter()
        .filter_map(|(used, flag)| used.then_some(*flag))
        .fold(String::new(), |mut acc, flag| {
            if !acc.is_empty() {
                acc.push(' ');
            }
            acc.push_str(flag);
            acc
        });
    if used.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{used} requires --flash <scatter>\nstandalone: pawflash disable-vbmeta\nexample: pawflash --flash <scatter.xml> --dry-run"
        ))
    }
}

/// Validate mutually-exclusive argument combinations and return an error if
/// the combination is invalid.
pub fn validate_args(args: &Args) -> Result<(), String> {
    if let Some(command) = &args.command {
        if args.force_fastboot {
            return Err("cannot combine a subcommand with --force-fastboot".to_string());
        }
        if args.set_active.is_some() {
            return Err("cannot combine a subcommand with --set-active".to_string());
        }
        if args.reboot_bootloader {
            return Err("cannot combine a subcommand with --reboot-bootloader".to_string());
        }
        if args.getvar_all {
            return Err("cannot combine a subcommand with --getvar-all".to_string());
        }
        if args.flash.is_some() {
            return Err("cannot combine a subcommand with --flash <scatter>".to_string());
        }
        if args.reboot {
            return Err("cannot combine a subcommand with --reboot".to_string());
        }
        if args.getvar.is_some() {
            return Err("cannot combine a subcommand with --getvar".to_string());
        }
        if args.firmware_upgrade
            || args.clean_flash
            || args.selective
            || args.slot.is_some()
            || args.include_preloader
        {
            return Err(
                "cannot combine a subcommand with legacy scatter flags; use the subcommand form"
                    .to_string(),
            );
        }

        if let Command::Scatter {
            firmware_upgrade,
            clean_flash,
            selective,
            ..
        } = command
        {
            validate_flash_mode_flags(args.dry_run, *firmware_upgrade, *clean_flash, *selective)?;
        }

        if let Command::Format { partition, .. } = command {
            if partition != "userdata" {
                return Err("format currently only supports `userdata`".to_string());
            }
        }
    }
    Ok(())
}

/// Ask whether to reboot to system after a successful scatter flash.
pub fn confirm_reboot_after_scatter() -> anyhow::Result<bool> {
    Confirm::new("Reboot to system now?")
        .with_default(false)
        .prompt()
        .context("confirm reboot after scatter flash")
}

/// Resolve a [`FlashMode`] from the four boolean flash-mode flags, validating
/// that at most one is set.
pub fn flash_mode_from_flags(
    dry_run: bool,
    firmware_upgrade: bool,
    clean_flash: bool,
    selective: bool,
) -> Result<FlashMode, String> {
    validate_flash_mode_flags(dry_run, firmware_upgrade, clean_flash, selective)?;

    if firmware_upgrade {
        Ok(FlashMode::FirmwareUpgrade)
    } else if clean_flash {
        Ok(FlashMode::CleanFlash)
    } else if selective {
        Ok(FlashMode::Selective)
    } else {
        Ok(FlashMode::DryRun)
    }
}

fn validate_flash_mode_flags(
    dry_run: bool,
    firmware_upgrade: bool,
    clean_flash: bool,
    selective: bool,
) -> Result<(), String> {
    let enabled = [dry_run, firmware_upgrade, clean_flash, selective]
        .into_iter()
        .filter(|enabled| *enabled)
        .count();
    if enabled > 1 {
        Err(
            "--dry-run, --firmware-upgrade, --clean-flash, and --selective are mutually exclusive"
                .to_string(),
        )
    } else {
        Ok(())
    }
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

/// Ask the user to confirm a scatter flash after previewing the plan.
pub fn confirm_scatter_plan(yes: bool) -> anyhow::Result<bool> {
    if yes {
        return Ok(true);
    }

    Confirm::new("Proceed with this scatter plan?")
        .with_default(false)
        .prompt()
        .context("confirm scatter plan")
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
