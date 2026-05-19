//! CLI argument parsing and validation using `clap`.

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

/// Fastboot-flasher subcommands.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Command {
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
    Reboot,
    /// Read a single fastboot variable.
    Getvar {
        /// Variable name to read.
        var: String,
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
        .collect::<Vec<_>>();
    if used.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} requires --flash <scatter>\nstandalone: fastboot-flasher disable-vbmeta\nexample: fastboot-flasher --flash <scatter.xml> --dry-run",
            used.join(" ")
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
