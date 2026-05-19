use clap::Parser;
use std::collections::HashMap;

use pawflash::{
    cli::{flash_mode_from_flags, validate_args, Args, Command},
    connect::connect_fastboot,
    device::{
        read_all_variables, read_variable, resolve_max_download_size_from_vars, reboot_device,
        reboot_device_bootloader, send_flashing_lock, send_flashing_unlock,
        set_fastboot_active_slot,
    },
    domain::{FlashEvent, FlashRunControl},
    format::{FormatTools, FormatUserdataOptions, WipeDataOptions},
    gsi::{execute_gsi_flash, GsiEvent, GsiFlashOptions},
    manual::{disable_vbmeta_actions, manual_flash_actions, resolved_disable_vbmeta_image_path},
    plan::build_plan_checked,
    workflow::{
        execute_manual_actions, format_userdata_flow, run_scatter_dry_run, run_scatter_flash,
        wipe_data_flow,
    },
};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    init_logging();
    let args = Args::parse();
    validate_args(&args).map_err(anyhow::Error::msg)?;
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

async fn run(args: Args) -> anyhow::Result<()> {
    match args.command {
        Some(Command::DisableVbmeta) => run_disable_vbmeta().await,
        Some(Command::Gsi { image }) => run_gsi(image).await,
        Some(Command::Format {
            partition,
            erase_fallback,
        }) => run_format(partition, erase_fallback).await,
        Some(Command::Scatter {
            scatter,
            firmware_upgrade,
            clean_flash,
            selective,
            slot,
            include_preloader,
        }) => {
            let mode = flash_mode_from_flags(
                false,
                firmware_upgrade,
                clean_flash,
                selective,
            )
            .map_err(anyhow::Error::msg)?;
            run_scatter(
                scatter,
                mode,
                slot,
                include_preloader,
                vec![],
                args.dry_run,
            )
            .await
        }
        Some(Command::Flash {
            partition,
            image,
            slot,
        }) => run_manual_flash(partition, image, slot).await,
        Some(Command::Reboot) => run_reboot().await,
        Some(Command::Getvar { var }) => run_getvar(var).await,
        Some(Command::UnlockBootloader) => run_unlock().await,
        Some(Command::LockBootloader) => run_lock().await,
        Some(Command::WipeData {
            no_metadata,
            no_cache,
            erase_fallback,
        }) => run_wipe_data(no_metadata, no_cache, erase_fallback).await,
        None => run_legacy(args).await,
    }
}

async fn run_legacy(args: Args) -> anyhow::Result<()> {
    if let Some(scatter) = args.flash.clone() {
        let mode = args.flash_mode();
        return run_scatter(
            scatter,
            mode,
            args.slot,
            args.include_preloader,
            vec![],
            args.dry_run,
        )
        .await;
    }

    if let Some(slot) = args.set_active {
        let mut dev = connect_fastboot().await?;
        let slot = match slot {
            pawflash::cli::SlotArg::A => "a",
            pawflash::cli::SlotArg::B => "b",
            pawflash::cli::SlotArg::Active => "active",
            pawflash::cli::SlotArg::Inactive => "inactive",
            pawflash::cli::SlotArg::All => "all",
        };
        set_fastboot_active_slot(&mut dev, slot).await?;
        return Ok(());
    }

    if let Some(var) = args.getvar {
        let mut dev = connect_fastboot().await?;
        let value = read_variable(&mut dev, &var).await?;
        println!("{value}");
        return Ok(());
    }

    if args.getvar_all {
        let mut dev = connect_fastboot().await?;
        let vars = read_all_variables(&mut dev).await?;
        println!("{}", serde_json::to_string_pretty(&vars)?);
        return Ok(());
    }

    if args.reboot {
        let mut dev = connect_fastboot().await?;
        reboot_device(&mut dev).await?;
        return Ok(());
    }

    if args.reboot_bootloader {
        let mut dev = connect_fastboot().await?;
        reboot_device_bootloader(&mut dev).await?;
        return Ok(());
    }

    if args.force_fastboot {
        let _ = pawflash::force_fastboot();
        return Ok(());
    }

    Ok(())
}

async fn run_scatter(
    scatter: std::path::PathBuf,
    mode: pawflash::cli::FlashMode,
    slot: Option<pawflash::cli::SlotArg>,
    include_preloader: bool,
    partitions: Vec<String>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let plan = build_plan_checked(&scatter, mode, slot, include_preloader, &partitions, true)?;
    let control = FlashRunControl::default();
    let mut emit = |event: FlashEvent| {
        print_flash_event(&event);
        Ok(())
    };
    let image_overrides = HashMap::new();

    if dry_run {
        let _summary = run_scatter_dry_run(&plan, &partitions, &control, &mut emit)
            .await
            .map_err(anyhow::Error::msg)?;
        return Ok(());
    }

    let mut dev = connect_fastboot().await?;
    let _summary = run_scatter_flash(
        &mut dev,
        &plan,
        &partitions,
        &image_overrides,
        false,
        &control,
        &mut emit,
    )
    .await
    .map_err(anyhow::Error::msg)?;
    Ok(())
}

async fn run_manual_flash(
    partition: String,
    image: std::path::PathBuf,
    slot: Option<pawflash::cli::SlotArg>,
) -> anyhow::Result<()> {
    let actions = manual_flash_actions(partition, image, slot)?;
    let control = FlashRunControl::default();
    let mut emit = |event: FlashEvent| {
        print_flash_event(&event);
        Ok(())
    };
    let mut dev = connect_fastboot().await?;
    let vars = read_all_variables(&mut dev).await?;
    let max_download = resolve_max_download_size_from_vars(&vars)?;
    let total_bytes: u64 = actions.iter().map(|a| a.size).sum();
    let mut summary = pawflash::domain::FlashSummaryDto {
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
        max_download,
        &control,
        &mut emit,
        &mut summary,
        total_bytes,
    )
    .await
    .map_err(anyhow::Error::msg)?;
    emit(FlashEvent::Complete {
        summary: summary.clone(),
    })
    .map_err(anyhow::Error::msg)?;
    Ok(())
}

async fn run_disable_vbmeta() -> anyhow::Result<()> {
    let image = resolved_disable_vbmeta_image_path()?;
    let actions = disable_vbmeta_actions(&image)?;
    let control = FlashRunControl::default();
    let mut emit = |event: FlashEvent| {
        print_flash_event(&event);
        Ok(())
    };
    let mut dev = connect_fastboot().await?;
    let vars = read_all_variables(&mut dev).await?;
    let max_download = resolve_max_download_size_from_vars(&vars)?;
    let total_bytes: u64 = actions.iter().map(|a| a.size).sum();
    let mut summary = pawflash::domain::FlashSummaryDto {
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
        max_download,
        &control,
        &mut emit,
        &mut summary,
        total_bytes,
    )
    .await
    .map_err(anyhow::Error::msg)?;
    emit(FlashEvent::Complete {
        summary: summary.clone(),
    })
    .map_err(anyhow::Error::msg)?;
    Ok(())
}

async fn run_format(partition: String, erase_fallback: bool) -> anyhow::Result<()> {
    anyhow::ensure!(partition == "userdata", "format currently only supports userdata");
    let control = FlashRunControl::default();
    let mut emit = |event: FlashEvent| {
        print_flash_event(&event);
        Ok(())
    };
    let mut dev = connect_fastboot().await?;
    let tools = FormatTools::from_cli_assets()?;
    let _summary = format_userdata_flow(
        &mut dev,
        &tools,
        &FormatUserdataOptions {
            erase_fallback,
            casefold: false,
        },
        &control,
        &mut emit,
    )
    .await
    .map_err(anyhow::Error::msg)?;
    Ok(())
}

async fn run_wipe_data(
    no_metadata: bool,
    no_cache: bool,
    erase_fallback: bool,
) -> anyhow::Result<()> {
    let control = FlashRunControl::default();
    let mut emit = |event: FlashEvent| {
        print_flash_event(&event);
        Ok(())
    };
    let mut dev = connect_fastboot().await?;
    let tools = FormatTools::from_cli_assets()?;
    let _summary = wipe_data_flow(
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
    Ok(())
}

async fn run_getvar(var: String) -> anyhow::Result<()> {
    let mut dev = connect_fastboot().await?;
    let value = read_variable(&mut dev, &var).await?;
    println!("{value}");
    Ok(())
}

async fn run_unlock() -> anyhow::Result<()> {
    let mut dev = connect_fastboot().await?;
    send_flashing_unlock(&mut dev).await?;
    Ok(())
}

async fn run_lock() -> anyhow::Result<()> {
    let mut dev = connect_fastboot().await?;
    send_flashing_lock(&mut dev).await?;
    Ok(())
}

async fn run_reboot() -> anyhow::Result<()> {
    let mut dev = connect_fastboot().await?;
    reboot_device(&mut dev).await?;
    Ok(())
}

async fn run_gsi(image: std::path::PathBuf) -> anyhow::Result<()> {
    let dev = connect_fastboot().await?;
    let tools = FormatTools::from_cli_assets()?;
    let control = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let options = GsiFlashOptions {
        wipe_data: WipeDataOptions::default(),
        cancel_token: Some(control),
    };
    let mut report = |event: GsiEvent| {
        println!("{event:?}");
    };
    let _outcome = execute_gsi_flash(dev, &image, &tools, &options, &mut report)
        .await
        .map_err(anyhow::Error::msg)?;
    Ok(())
}

fn print_flash_event(event: &FlashEvent) {
    match event {
        FlashEvent::WaitingForDevice => println!("waiting for device"),
        FlashEvent::PlanBuilt {
            actions,
            total_bytes,
        } => println!("plan built: {actions} actions, {total_bytes} bytes"),
        FlashEvent::PreparingImage { partition } => println!("preparing {partition}"),
        FlashEvent::Flashing {
            partition,
            bytes,
            total,
            ..
        } => println!("flashing {partition}: {bytes}/{total}"),
        FlashEvent::Simulating {
            partition,
            action,
            bytes,
            total,
            ..
        } => println!("simulating {action} {partition}: {bytes}/{total}"),
        FlashEvent::PartitionComplete { partition } => println!("complete {partition}"),
        FlashEvent::PartitionSkipped { partition, reason } => {
            println!("skipped {partition}: {reason}")
        }
        FlashEvent::PartitionFailed { partition, error } => {
            eprintln!("failed {partition}: {error}")
        }
        FlashEvent::Erasing { partition } => println!("erasing {partition}"),
        FlashEvent::EraseComplete { partition } => println!("erased {partition}"),
        FlashEvent::Overall { bytes, total } => println!("overall {bytes}/{total}"),
        FlashEvent::Complete { summary } => println!(
            "done: flash={} wipe={} skipped={} total={}",
            summary.flash_count, summary.wipe_count, summary.skipped_count, summary.total_bytes
        ),
        FlashEvent::Cancelled { message } => eprintln!("{message}"),
        FlashEvent::Error { message } => eprintln!("{message}"),
        FlashEvent::DeviceCheckDiagnostic { stage, level, message } => {
            println!("[{level}] {stage}: {message}")
        }
        FlashEvent::GsiStatus { status } => println!("gsi: {status}"),
    }
}
