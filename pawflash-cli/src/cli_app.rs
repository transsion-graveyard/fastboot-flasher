use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use pawflash::cli::{FlashMode, RebootTargetArg, SlotArg};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
    Human,
    Machine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FlashModeArg {
    DryRun,
    DirtyFlash,
    CleanFlash,
    Selective,
}

impl From<FlashModeArg> for FlashMode {
    fn from(value: FlashModeArg) -> Self {
        match value {
            FlashModeArg::DryRun => FlashMode::DryRun,
            FlashModeArg::DirtyFlash => FlashMode::DirtyFlash,
            FlashModeArg::CleanFlash => FlashMode::CleanFlash,
            FlashModeArg::Selective => FlashMode::Selective,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SlotArgValue {
    A,
    B,
    Active,
    Inactive,
    All,
}

impl From<SlotArgValue> for SlotArg {
    fn from(value: SlotArgValue) -> Self {
        match value {
            SlotArgValue::A => SlotArg::A,
            SlotArgValue::B => SlotArg::B,
            SlotArgValue::Active => SlotArg::Active,
            SlotArgValue::Inactive => SlotArg::Inactive,
            SlotArgValue::All => SlotArg::All,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RebootTargetArgValue {
    System,
    Bootloader,
    Fastboot,
    Recovery,
}

impl From<RebootTargetArgValue> for RebootTargetArg {
    fn from(value: RebootTargetArgValue) -> Self {
        match value {
            RebootTargetArgValue::System => RebootTargetArg::System,
            RebootTargetArgValue::Bootloader => RebootTargetArg::Bootloader,
            RebootTargetArgValue::Fastboot => RebootTargetArg::Fastboot,
            RebootTargetArgValue::Recovery => RebootTargetArg::Recovery,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "pawflash",
    about = "Modern MTK fastboot workflow runner",
    arg_required_else_help = true
)]
pub struct AppArgs {
    #[arg(long, global = true)]
    pub non_interactive: bool,
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Human)]
    pub output: OutputFormat,
    #[arg(long, global = true)]
    pub quiet: bool,
    #[arg(long, global = true)]
    pub verbose: bool,
    #[command(subcommand)]
    pub command: TopLevelCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum TopLevelCommand {
    Device(DeviceArgs),
    Inspect(InspectArgs),
    Flash(FlashArgs),
    Wipe(WipeArgs),
    Bootloader(BootloaderArgs),
    Reboot(RebootCommand),
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum DeviceCommand {
    Status,
    Var { name: String },
    Vars,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct DeviceArgs {
    #[command(subcommand)]
    pub command: DeviceCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum InspectCommand {
    Plan {
        scatter: PathBuf,
        #[arg(long, value_enum, default_value_t = FlashModeArg::DryRun)]
        mode: FlashModeArg,
        #[arg(long, value_enum)]
        slot: Option<SlotArgValue>,
        #[arg(long)]
        include_preloader: bool,
    },
    Package {
        scatter: PathBuf,
    },
    Device,
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct InspectArgs {
    #[command(subcommand)]
    pub command: InspectCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum FlashCommand {
    Package {
        scatter: PathBuf,
        #[arg(long, value_enum, default_value_t = FlashModeArg::CleanFlash)]
        mode: FlashModeArg,
        #[arg(long, value_enum)]
        slot: Option<SlotArgValue>,
        #[arg(long)]
        include_preloader: bool,
        #[arg(long)]
        reboot: bool,
    },
    Partition {
        partition: String,
        image: PathBuf,
        #[arg(long, value_enum)]
        slot: Option<SlotArgValue>,
        #[arg(long)]
        reboot: bool,
    },
    Gsi {
        image: PathBuf,
    },
    Vbmeta {
        #[command(subcommand)]
        command: VbmetaCommand,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct FlashArgs {
    #[command(subcommand)]
    pub command: FlashCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum VbmetaCommand {
    Disable,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum WipeCommand {
    Data {
        #[arg(long)]
        no_metadata: bool,
        #[arg(long)]
        no_cache: bool,
        #[arg(long)]
        erase_fallback: bool,
    },
    FormatUserdata {
        #[arg(long)]
        erase_fallback: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct WipeArgs {
    #[command(subcommand)]
    pub command: WipeCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum BootloaderCommand {
    ForceFastboot,
    Unlock,
    Lock,
    Slot {
        #[command(subcommand)]
        command: BootloaderSlotCommand,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct BootloaderArgs {
    #[command(subcommand)]
    pub command: BootloaderCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum BootloaderSlotCommand {
    Set { slot: SlotArgValue },
}

#[derive(Debug, Clone, PartialEq, Eq, clap::Args)]
pub struct RebootCommand {
    #[arg(value_enum, default_value_t = RebootTargetArgValue::System)]
    pub target: RebootTargetArgValue,
}

impl AppArgs {
    pub fn ui_mode(&self, stdout_is_terminal: bool) -> UiMode {
        if self.non_interactive || self.output == OutputFormat::Json || !stdout_is_terminal {
            UiMode::Machine
        } else {
            UiMode::Human
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_grouped_flash_package_command() {
        let args = AppArgs::parse_from([
            "pawflash",
            "flash",
            "package",
            "firmware/MT6789_Android_scatter.xml",
            "--mode",
            "clean-flash",
            "--slot",
            "inactive",
            "--include-preloader",
            "--reboot",
        ]);

        assert_eq!(
            args.command,
            TopLevelCommand::Flash(FlashArgs {
                command: FlashCommand::Package {
                    scatter: PathBuf::from("firmware/MT6789_Android_scatter.xml"),
                    mode: FlashModeArg::CleanFlash,
                    slot: Some(SlotArgValue::Inactive),
                    include_preloader: true,
                    reboot: true,
                }
            })
        );
    }

    #[test]
    fn parses_inspect_plan_command() {
        let args = AppArgs::parse_from([
            "pawflash",
            "inspect",
            "plan",
            "firmware/MT6789_Android_scatter.xml",
            "--mode",
            "selective",
        ]);

        assert_eq!(
            args.command,
            TopLevelCommand::Inspect(InspectArgs {
                command: InspectCommand::Plan {
                    scatter: PathBuf::from("firmware/MT6789_Android_scatter.xml"),
                    mode: FlashModeArg::Selective,
                    slot: None,
                    include_preloader: false,
                }
            })
        );
    }

    #[test]
    fn parses_dirty_flash_mode_for_flash_package() {
        let args = AppArgs::parse_from([
            "pawflash",
            "flash",
            "package",
            "firmware/MT6789_Android_scatter.xml",
            "--mode",
            "dirty-flash",
        ]);

        assert_eq!(
            args.command,
            TopLevelCommand::Flash(FlashArgs {
                command: FlashCommand::Package {
                    scatter: PathBuf::from("firmware/MT6789_Android_scatter.xml"),
                    mode: FlashModeArg::DirtyFlash,
                    slot: None,
                    include_preloader: false,
                    reboot: false,
                }
            })
        );
    }

    #[test]
    fn rejects_legacy_firmware_upgrade_mode_name() {
        let error = AppArgs::try_parse_from([
            "pawflash",
            "flash",
            "package",
            "firmware/MT6789_Android_scatter.xml",
            "--mode",
            "firmware-upgrade",
        ])
        .unwrap_err();

        let rendered = error.to_string();
        assert!(rendered.contains("invalid value 'firmware-upgrade'"));
    }

    #[test]
    fn flash_package_defaults_to_clean_flash() {
        let args = AppArgs::parse_from([
            "pawflash",
            "flash",
            "package",
            "firmware/MT6789_Android_scatter.xml",
        ]);

        assert_eq!(
            args.command,
            TopLevelCommand::Flash(FlashArgs {
                command: FlashCommand::Package {
                    scatter: PathBuf::from("firmware/MT6789_Android_scatter.xml"),
                    mode: FlashModeArg::CleanFlash,
                    slot: None,
                    include_preloader: false,
                    reboot: false,
                }
            })
        );
    }

    #[test]
    fn machine_mode_is_selected_for_json_output() {
        let args = AppArgs::parse_from(["pawflash", "--output", "json", "device", "vars"]);

        assert_eq!(args.ui_mode(true), UiMode::Machine);
    }

    #[test]
    fn machine_mode_is_selected_for_non_interactive_runs() {
        let args = AppArgs::parse_from([
            "pawflash",
            "--non-interactive",
            "flash",
            "vbmeta",
            "disable",
        ]);

        assert_eq!(args.ui_mode(true), UiMode::Machine);
    }
}
