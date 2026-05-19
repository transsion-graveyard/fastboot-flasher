use std::path::PathBuf;

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashMode {
    DryRun,
    FirmwareUpgrade,
    CleanFlash,
    Selective,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SlotArg {
    A,
    B,
    Active,
    Inactive,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Flash the bundled empty vbmeta image to vbmeta_a and vbmeta_b.
    DisableVbmeta,
    /// Flash a GSI system image using the app's GSI flow.
    Gsi {
        image: PathBuf,
    },
    /// Format userdata like recovery's "Format Data".
    Format {
        partition: String,
        #[arg(long)]
        erase_fallback: bool,
    },
    /// Build a scatter plan and optionally execute it.
    Scatter {
        scatter: PathBuf,
        #[arg(long)]
        firmware_upgrade: bool,
        #[arg(long)]
        clean_flash: bool,
        #[arg(long)]
        selective: bool,
        #[arg(long, value_enum)]
        slot: Option<SlotArg>,
        #[arg(long)]
        include_preloader: bool,
    },
    /// Flash one image to one exact fastboot partition.
    Flash {
        partition: String,
        image: PathBuf,
        #[arg(long, value_enum)]
        slot: Option<SlotArg>,
    },
    /// Reboot the current fastboot device.
    Reboot,
    /// Read a single fastboot variable.
    Getvar { var: String },
    /// Send `flashing unlock`.
    UnlockBootloader,
    /// Send `flashing lock`.
    LockBootloader,
    /// Format userdata, then best-effort erase metadata and cache.
    WipeData {
        #[arg(long)]
        no_metadata: bool,
        #[arg(long)]
        no_cache: bool,
        #[arg(long)]
        erase_fallback: bool,
    },
}

#[derive(Debug, Parser)]
#[command(about = "Plan and execute safe MTK fastboot flashing flows")]
#[command(group(ArgGroup::new("flash_mode").args([
    "dry_run",
    "firmware_upgrade",
    "clean_flash",
    "selective",
])))]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,
    #[arg(long)]
    pub force_fastboot: bool,
    #[arg(long)]
    pub flash: Option<PathBuf>,
    #[arg(long, global = true)]
    pub dry_run: bool,
    #[arg(long, requires = "flash")]
    pub firmware_upgrade: bool,
    #[arg(long, requires = "flash")]
    pub clean_flash: bool,
    #[arg(long, requires = "flash")]
    pub selective: bool,
    #[arg(long, value_enum, requires = "flash")]
    pub slot: Option<SlotArg>,
    #[arg(long, requires = "flash")]
    pub include_preloader: bool,
    #[arg(long, global = true)]
    pub yes: bool,
    #[arg(long)]
    pub set_active: Option<SlotArg>,
    #[arg(long)]
    pub reboot: bool,
    #[arg(long)]
    pub reboot_bootloader: bool,
    #[arg(long)]
    pub getvar: Option<String>,
    #[arg(long)]
    pub getvar_all: bool,
}

impl Args {
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
