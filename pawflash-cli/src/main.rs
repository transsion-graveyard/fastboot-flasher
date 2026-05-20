use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use clap::Parser;

use pawflash::device_info::compact_device_info;
use pawflash::FastbootDevice;
use pawflash::{
    cli::{scatter_plan_preview_lines, FlashMode, RebootTargetArg, SlotArg},
    connect::{connect_fastboot, try_connect_fastboot},
    device::{
        read_all_variables, read_variable, reboot_device, reboot_device_bootloader,
        reboot_device_fastboot, resolve_flash_partition_target,
        resolve_max_download_size_from_vars, send_flashing_lock, send_flashing_unlock,
        set_fastboot_active_slot,
    },
    domain::{plan_to_dto, FlashEvent, FlashRunControl, FlashSummaryDto},
    format::{FormatTools, WipeDataOptions},
    gsi::{execute_gsi_flash, GsiEvent, GsiFlashOptions},
    manual::{disable_vbmeta_actions, manual_flash_actions, resolved_disable_vbmeta_image_path},
    plan::build_plan_checked,
    workflow::{
        execute_manual_actions, run_scatter_dry_run, run_scatter_flash, wipe_data_flow,
        ManualActionExecution, ScatterFlashOptions,
    },
};

mod cli_app;
mod progress;
mod ui;

use terminal_output::spinner::StatusSpinner;

use crate::cli_app::{
    AppArgs, BootloaderArgs, BootloaderCommand, BootloaderSlotCommand, DataArgs, DataCommand,
    DeviceArgs, DeviceCommand, FlashArgs, FlashCommand, InspectArgs, InspectCommand, OutputFormat,
    TopLevelCommand, UiMode, VbmetaCommand,
};
use crate::ui::Session;

struct ScatterRunRequest {
    scatter: PathBuf,
    mode: FlashMode,
    slot: Option<SlotArg>,
    include_preloader: bool,
    partitions: Vec<String>,
    reboot: bool,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    init_logging();
    let args = AppArgs::parse();
    run(args).await
}

fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();
}

async fn run(args: AppArgs) -> anyhow::Result<()> {
    let ui_mode = args.ui_mode(std::io::stdout().is_terminal());
    let session = Session::new(ui_mode, args.output);

    session.intro("pawflash")?;

    let result = match args.command {
        TopLevelCommand::Device(command) => run_device(&session, command).await,
        TopLevelCommand::Inspect(command) => run_inspect(&session, command).await,
        TopLevelCommand::Flash(command) => run_flash(&session, command).await,
        TopLevelCommand::Data(command) => run_data(&session, command).await,
        TopLevelCommand::Bootloader(command) => run_bootloader(&session, command).await,
        TopLevelCommand::Reboot(command) => {
            run_reboot_command(&session, command.target.into()).await
        }
    };

    match result {
        Ok(()) => {
            session.finish_success("workflow complete")?;
            Ok(())
        }
        Err(error) => {
            session.finish_cancelled(error.to_string())?;
            Err(error)
        }
    }
}

async fn run_device(session: &Session, args: DeviceArgs) -> anyhow::Result<()> {
    match args.command {
        DeviceCommand::Status => {
            let mut dev = connect_with_spinner().await?;
            let vars = read_all_variables(&mut dev).await?;
            if session.mode() == UiMode::Machine {
                session.emit_json(&vars)?;
            } else {
                println!("{}", compact_device_info(&vars));
            }
        }
        DeviceCommand::Var { name } => {
            let mut dev = connect_with_spinner().await?;
            let value = read_variable(&mut dev, &name).await?;
            match session.mode() {
                UiMode::Machine => session.emit_json(&serde_json::json!({
                    "name": name,
                    "value": value,
                }))?,
                UiMode::Human => println!("{value}"),
            }
        }
        DeviceCommand::Vars => {
            let mut dev = connect_with_spinner().await?;
            let vars = read_all_variables(&mut dev).await?;
            match session.output() {
                OutputFormat::Json => session.emit_json(&vars)?,
                OutputFormat::Human => println!("{}", serde_json::to_string_pretty(&vars)?),
            }
        }
    }

    Ok(())
}

async fn run_inspect(session: &Session, args: InspectArgs) -> anyhow::Result<()> {
    match args.command {
        InspectCommand::Plan {
            scatter,
            mode,
            slot,
            include_preloader,
        } => {
            let plan = build_plan_checked(
                &scatter,
                mode.into(),
                slot.map(Into::into),
                include_preloader,
                &[],
                true,
            )?;
            if session.output() == OutputFormat::Json {
                let dto = plan_to_dto(&plan, None);
                session.emit_json(&dto)?;
            } else {
                session.render_plan_summary(&plan)?;
                for line in scatter_plan_preview_lines(&plan) {
                    println!("{line}");
                }
            }
        }
        InspectCommand::Package { scatter } => {
            let plan = build_plan_checked(&scatter, FlashMode::DryRun, None, false, &[], true)?;
            if session.output() == OutputFormat::Json {
                let dto = plan_to_dto(&plan, None);
                session.emit_json(&dto)?;
            } else {
                session.render_plan_summary(&plan)?;
                for line in scatter_plan_preview_lines(&plan) {
                    println!("{line}");
                }
            }
        }
        InspectCommand::Device => {
            let mut dev = connect_with_spinner().await?;
            let vars = read_all_variables(&mut dev).await?;
            match session.output() {
                OutputFormat::Json => session.emit_json(&vars)?,
                OutputFormat::Human => println!("{}", compact_device_info(&vars)),
            }
        }
    }

    Ok(())
}

async fn run_flash(session: &Session, args: FlashArgs) -> anyhow::Result<()> {
    match args.command {
        FlashCommand::Package {
            scatter,
            mode,
            slot,
            include_preloader,
            reboot,
        } => {
            let request = ScatterRunRequest {
                scatter,
                mode: mode.into(),
                slot: slot.map(Into::into),
                include_preloader,
                partitions: Vec::new(),
                reboot,
            };
            run_scatter(session, request).await
        }
        FlashCommand::Partition {
            partition,
            image,
            slot,
            reboot: _,
        } => run_manual_flash(session, partition, image, slot.map(Into::into)).await,
        FlashCommand::Gsi { image } => run_gsi(session, image).await,
        FlashCommand::Vbmeta { command } => match command {
            VbmetaCommand::Disable => run_disable_vbmeta(session).await,
        },
    }
}

async fn run_data(session: &Session, args: DataArgs) -> anyhow::Result<()> {
    match args.command {
        DataCommand::Format {
            no_metadata,
            no_cache,
            erase_fallback,
        } => run_format_data(session, no_metadata, no_cache, erase_fallback).await,
    }
}

async fn run_bootloader(session: &Session, args: BootloaderArgs) -> anyhow::Result<()> {
    match args.command {
        BootloaderCommand::ForceFastboot => run_force_fastboot(session),
        BootloaderCommand::Unlock => {
            let mut dev = connect_with_spinner().await?;
            send_flashing_unlock(&mut dev).await?;
            Ok(())
        }
        BootloaderCommand::Lock => {
            let mut dev = connect_with_spinner().await?;
            send_flashing_lock(&mut dev).await?;
            Ok(())
        }
        BootloaderCommand::Slot { command } => match command {
            BootloaderSlotCommand::Set { slot } => {
                let mut dev = connect_with_spinner().await?;
                set_fastboot_active_slot(&mut dev, slot_name(slot.into())).await?;
                Ok(())
            }
        },
    }
}

async fn run_reboot_command(session: &Session, target: RebootTargetArg) -> anyhow::Result<()> {
    let _ = session;
    let mut dev = connect_with_spinner().await?;
    match target {
        RebootTargetArg::System => reboot_device(&mut dev).await?,
        RebootTargetArg::Bootloader => reboot_device_bootloader(&mut dev).await?,
        RebootTargetArg::Fastboot => reboot_device_fastboot(&mut dev).await?,
        RebootTargetArg::Recovery => dev.reboot_to("recovery").await?,
    }
    Ok(())
}

async fn run_scatter(session: &Session, request: ScatterRunRequest) -> anyhow::Result<()> {
    let plan = build_plan_checked(
        &request.scatter,
        request.mode,
        request.slot,
        request.include_preloader,
        &request.partitions,
        true,
    )?;
    let control = FlashRunControl::default();
    let mut emit = make_flash_emit();
    let image_overrides = HashMap::new();

    if session.output() == OutputFormat::Json {
        let dto = plan_to_dto(&plan, None);
        session.emit_json(&dto)?;
    } else {
        session.render_plan_summary(&plan)?;
    }

    if request.mode == FlashMode::DryRun {
        let summary = run_scatter_dry_run(&plan, &request.partitions, &control, &mut emit)
            .await
            .map_err(anyhow::Error::msg)?;
        finish_summary(session, &summary)?;
        return Ok(());
    }

    ensure_device_or_offer_force_fastboot(session).await?;

    if session.mode() == UiMode::Human && !session.confirm("Proceed with flash plan?", false)? {
        return Ok(());
    }

    let tools = FormatTools::from_cli_assets()?;
    let mut dev = connect_with_spinner().await?;
    let summary = run_scatter_flash(
        &mut dev,
        &plan,
        ScatterFlashOptions {
            partitions: &request.partitions,
            image_overrides: &image_overrides,
            announce_plan: false,
            reboot: request.reboot,
            format_tools: Some(&tools),
            control: &control,
        },
        &mut emit,
    )
    .await
    .map_err(anyhow::Error::msg)?;

    if !request.reboot
        && session.mode() == UiMode::Human
        && session.confirm("Reboot to system now?", false)?
    {
        reboot_device(&mut dev).await?;
    }

    finish_summary(session, &summary)
}

async fn run_manual_flash(
    session: &Session,
    partition: String,
    image: PathBuf,
    slot: Option<SlotArg>,
) -> anyhow::Result<()> {
    ensure_device_or_offer_force_fastboot(session).await?;

    if session.mode() == UiMode::Human
        && !session.confirm("Proceed with direct partition flash?", false)?
    {
        return Ok(());
    }

    let actions = manual_flash_actions(partition, image, slot)?;
    let control = FlashRunControl::default();
    let mut emit = make_flash_emit();
    let mut dev = connect_with_spinner().await?;
    let vars = read_all_variables(&mut dev).await?;
    let max_download = resolve_max_download_size_from_vars(&vars)?;
    let total_bytes: u64 = actions.iter().map(|action| action.size).sum();
    let mut summary = FlashSummaryDto {
        flash_count: 0,
        wipe_count: 0,
        skipped_count: 0,
        total_bytes,
    };

    emit(FlashEvent::PlanBuilt {
        actions: actions.len(),
        total_bytes,
    })
    .map_err(anyhow::Error::msg)?;
    emit(FlashEvent::Overall {
        bytes: 0,
        total: total_bytes,
    })
    .map_err(anyhow::Error::msg)?;

    execute_manual_actions(
        &actions,
        &mut dev,
        ManualActionExecution {
            max_download_size: max_download,
            partition_resolver: &|partition| partition.to_string(),
            control: &control,
            summary: &mut summary,
            overall_total: total_bytes,
        },
        &mut emit,
    )
    .await
    .map_err(anyhow::Error::msg)?;
    emit(FlashEvent::Complete {
        summary: summary.clone(),
    })
    .map_err(anyhow::Error::msg)?;

    finish_summary(session, &summary)
}

async fn run_disable_vbmeta(session: &Session) -> anyhow::Result<()> {
    ensure_device_or_offer_force_fastboot(session).await?;

    if session.mode() == UiMode::Human
        && !session.confirm("Disable vbmeta verification on both slots?", false)?
    {
        return Ok(());
    }

    let image = resolved_disable_vbmeta_image_path()?;
    let control = FlashRunControl::default();
    let mut emit = make_flash_emit();
    let mut dev = connect_with_spinner().await?;
    let vars = read_all_variables(&mut dev).await?;
    let max_download = resolve_max_download_size_from_vars(&vars)?;
    let actions = disable_vbmeta_actions(&image)?;
    let total_bytes: u64 = actions.iter().map(|action| action.size).sum();
    let mut summary = FlashSummaryDto {
        flash_count: 0,
        wipe_count: 0,
        skipped_count: 0,
        total_bytes,
    };

    emit(FlashEvent::PlanBuilt {
        actions: actions.len(),
        total_bytes,
    })
    .map_err(anyhow::Error::msg)?;
    emit(FlashEvent::Overall {
        bytes: 0,
        total: total_bytes,
    })
    .map_err(anyhow::Error::msg)?;

    execute_manual_actions(
        &actions,
        &mut dev,
        ManualActionExecution {
            max_download_size: max_download,
            partition_resolver: &|partition| resolve_flash_partition_target(partition, &vars),
            control: &control,
            summary: &mut summary,
            overall_total: total_bytes,
        },
        &mut emit,
    )
    .await
    .map_err(anyhow::Error::msg)?;
    emit(FlashEvent::Complete {
        summary: summary.clone(),
    })
    .map_err(anyhow::Error::msg)?;

    finish_summary(session, &summary)
}

async fn run_format_data(
    session: &Session,
    no_metadata: bool,
    no_cache: bool,
    erase_fallback: bool,
) -> anyhow::Result<()> {
    ensure_device_or_offer_force_fastboot(session).await?;
    if session.mode() == UiMode::Human
        && !session.confirm("Format data and clear optional partitions?", false)?
    {
        return Ok(());
    }

    let control = FlashRunControl::default();
    let mut emit = make_flash_emit();
    let mut dev = connect_with_spinner().await?;
    let tools = FormatTools::from_cli_assets()?;
    let summary = wipe_data_flow(
        &mut dev,
        &tools,
        &WipeDataOptions {
            erase_metadata: !no_metadata,
            erase_cache: !no_cache,
            erase_fallback,
            casefold: false,
        },
        &control,
        &mut emit,
    )
    .await
    .map_err(anyhow::Error::msg)?;

    finish_summary(session, &summary)
}

fn run_force_fastboot(_session: &Session) -> anyhow::Result<()> {
    pawflash::run_force_fastboot_quiet(&pawflash::ForceFastbootOptions::default())
        .map_err(anyhow::Error::from)
}

async fn run_gsi(session: &Session, image: PathBuf) -> anyhow::Result<()> {
    ensure_device_or_offer_force_fastboot(session).await?;

    if session.mode() == UiMode::Human
        && !session.confirm("Proceed with the GSI flash workflow?", false)?
    {
        return Ok(());
    }

    let dev = connect_with_spinner().await?;
    let tools = FormatTools::from_cli_assets()?;
    let control = Arc::new(AtomicBool::new(false));
    let options = GsiFlashOptions {
        wipe_data: WipeDataOptions::default(),
        cancel_token: Some(control),
    };
    let mut report = |event: GsiEvent| match event {
        GsiEvent::Step(step) => println!("gsi: {}", step.as_str()),
        GsiEvent::ModeDetected(mode) => println!("gsi mode: {}", mode.as_str()),
        GsiEvent::ModeReady(mode) => println!("gsi ready: {}", mode.as_str()),
        GsiEvent::Flashing {
            partition,
            size_bytes,
            ..
        } => println!(
            "gsi flashing {partition}: {}",
            indicatif::HumanBytes(size_bytes)
        ),
        GsiEvent::FlashFinished {
            partition,
            size_bytes,
        } => println!(
            "gsi flashed {partition}: {}",
            indicatif::HumanBytes(size_bytes)
        ),
        GsiEvent::Erasing { partition } => println!("gsi erasing {partition}"),
        GsiEvent::EraseFinished { partition } => println!("gsi erased {partition}"),
        GsiEvent::PartitionSkipped { partition, reason } => {
            println!("gsi skipped {partition}: {reason}")
        }
        GsiEvent::UserdataEraseFallback { fs_type } => {
            println!("gsi data erase fallback: {fs_type}")
        }
        GsiEvent::ResolvedPartition {
            base,
            partition,
            size_bytes,
        } => println!(
            "gsi resolved {base} -> {partition} ({})",
            indicatif::HumanBytes(size_bytes)
        ),
        GsiEvent::FlashProgress {
            partition,
            bytes,
            total_bytes,
            ..
        } => println!("gsi {partition}: {bytes}/{total_bytes}"),
    };
    let outcome = execute_gsi_flash(dev, &image, &tools, &options, &mut report)
        .await
        .map_err(anyhow::Error::msg)?;

    let summary = FlashSummaryDto {
        flash_count: outcome.summary.flash_count,
        wipe_count: outcome.summary.wipe_count,
        skipped_count: outcome.summary.skipped_count,
        total_bytes: outcome.summary.total_bytes,
    };
    finish_summary(session, &summary)
}

async fn ensure_device_or_offer_force_fastboot(session: &Session) -> anyhow::Result<()> {
    if try_connect_fastboot().await.is_ok() {
        return Ok(());
    }

    if session.mode() == UiMode::Human {
        session.note(
            "Device",
            "No fastboot device is ready. You can connect one now or let pawflash try force-fastboot.",
        )?;
        if session.confirm("Try force-fastboot now?", true)? {
            run_force_fastboot(session)?;
        }
    }

    Ok(())
}

fn finish_summary(session: &Session, summary: &FlashSummaryDto) -> anyhow::Result<()> {
    if session.output() == OutputFormat::Json {
        session.emit_json(summary)
    } else {
        session.render_run_summary(summary)
    }
}

async fn connect_with_spinner() -> anyhow::Result<FastbootDevice> {
    let _spinner = StatusSpinner::new("Waiting for fastboot device...");
    connect_fastboot().await
}

fn slot_name(slot: SlotArg) -> &'static str {
    match slot {
        SlotArg::A => "a",
        SlotArg::B => "b",
        SlotArg::Active => "active",
        SlotArg::Inactive => "inactive",
        SlotArg::All => "all",
    }
}

fn print_flash_event(event: &FlashEvent) {
    match event {
        FlashEvent::WaitingForDevice => println!("waiting for device"),
        FlashEvent::PlanBuilt {
            actions,
            total_bytes,
        } => println!(
            "plan built: {actions} actions, {}",
            indicatif::HumanBytes(*total_bytes)
        ),
        FlashEvent::PreparingImage {
            partition,
            operation,
        } => println!("preparing {operation:?} {partition}"),
        FlashEvent::Flashing {
            partition,
            operation,
            bytes,
            total,
            ..
        } => println!("{operation:?} {partition}: {bytes}/{total}"),
        FlashEvent::Simulating {
            partition,
            operation,
            bytes,
            total,
            ..
        } => println!("simulating {operation:?} {partition}: {bytes}/{total}"),
        FlashEvent::PartitionComplete {
            partition,
            operation,
        } => println!("complete {operation:?} {partition}"),
        FlashEvent::PartitionSkipped {
            partition,
            operation,
            reason,
        } => println!("skipped {operation:?} {partition}: {reason}"),
        FlashEvent::PartitionFailed {
            partition,
            operation,
            error,
        } => eprintln!("failed {operation:?} {partition}: {error}"),
        FlashEvent::Erasing { partition } => println!("erasing {partition}"),
        FlashEvent::EraseComplete { partition } => println!("erased {partition}"),
        FlashEvent::Overall { bytes, total } => println!("overall {bytes}/{total}"),
        FlashEvent::Complete { summary } => println!(
            "done: flash={} wipe={} skipped={} total={}",
            summary.flash_count,
            summary.wipe_count,
            summary.skipped_count,
            indicatif::HumanBytes(summary.total_bytes)
        ),
        FlashEvent::Cancelled { message } => eprintln!("{message}"),
        FlashEvent::Error { message } => eprintln!("{message}"),
        FlashEvent::DeviceCheckDiagnostic {
            stage,
            level,
            message,
        } => println!("[{level}] {stage}: {message}"),
        FlashEvent::GsiStatus { status } => println!("gsi: {status}"),
        FlashEvent::Rebooting { target } => println!("rebooting to {target}"),
    }
}

fn make_flash_emit() -> impl FnMut(FlashEvent) -> Result<(), String> {
    let mut renderer = progress::CliProgressRenderer::new();
    move |event: FlashEvent| {
        if !renderer.handle(&event) {
            print_flash_event(&event);
        }
        Ok(())
    }
}
