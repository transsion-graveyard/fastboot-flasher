#![allow(missing_docs)]

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{bail, Context};
use clap::{CommandFactory, Parser};
use fastboot_rs::{flash_prepared_image, prepare_image, FastbootDevice, FlashProgress};

use fastboot_flasher::{
    build_flash_plan,
    cli::{
        flash_mode_from_flags, flash_modifier_without_flash, validate_args, Args, Command,
        FlashMode, SlotArg,
    },
    connect_fastboot,
    device::{compact_device_info, mock_device_info},
    format::{
        detect_userdata, format_userdata_with_info, wipe_data_with_info, FormatTools,
        FormatUserdataOptions, WipeDataOptions,
    },
    gsi::{
        build_gsi_execution_plan, detect_fastboot_mode, execute_gsi_flash_with_vars,
        inspect_gsi_image, maybe_needs_product_gsi, GsiEvent, GsiFlashOptions,
    },
    handle_failed_erase, handle_failed_partition,
    manual::{
        disable_vbmeta_actions, manual_flash_actions, standalone_disable_vbmeta_path,
        ManualFlashAction,
    },
    plan::mode_to_scatter,
    progress::{
        action_summary, active_action_label, dry_run_steps, erase_history_message, fit_width,
        flash_history_message, flash_history_min_width, format_byte_pair, format_mm_ss,
        max_visible_width, progress_header, selective_option_label,
        should_confirm_before_simulation, skipped_erase_history_message,
        skipped_flash_history_message, skipped_flash_history_min_width, visible_width,
        ActionSummary,
    },
};
use force_fastboot::{run_force_fastboot, ForceFastbootOptions};
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressState, ProgressStyle};
use inquire::{Confirm, MultiSelect};
use mtk_scatter_parser::FlashPlan;
use terminal_output::chrome::{simple_banner, simple_notice_box, simple_section_header, simple_status_line, Tone};
use terminal_output::table::simple_kv_table;
use tokio::time::sleep;

const DRY_RUN_SPEED_MIB: u64 = 1024;

#[derive(Debug, Clone)]
enum Action {
    ForceFastboot,
    SetActive(SlotArg),
    GetVar(String),
    GetVarAll,
    Scatter {
        scatter: PathBuf,
        mode: FlashMode,
        slot: Option<SlotArg>,
        include_preloader: bool,
    },
    DisableVbmeta,
    Gsi {
        image: PathBuf,
    },
    FormatUserdata {
        erase_fallback: bool,
    },
    Flash {
        partition: String,
        image: PathBuf,
        slot: Option<SlotArg>,
    },
    Reboot,
    RebootBootloader,
    UnlockBootloader,
    LockBootloader,
    WipeData {
        no_metadata: bool,
        no_cache: bool,
        erase_fallback: bool,
    },
}

#[tokio::main]
async fn main() {
    let code = match run().await {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{}", simple_notice_box(Tone::Error, "fatal", &format!("{err:#}")));
            1
        }
    };
    std::process::exit(code);
}

async fn run() -> anyhow::Result<()> {
    let args = Args::parse();
    if let Err(message) = validate_args(&args) {
        bail!("{message}");
    }
    if let Err(message) = flash_modifier_without_flash(&args) {
        bail!("{message}");
    }
    let actions = collect_actions(&args)?;

    if actions.is_empty() {
        Args::command().print_help()?;
        println!();
        return Ok(());
    }

    for action in actions {
        match action {
            Action::ForceFastboot => {
                run_force_fastboot(&ForceFastbootOptions {
                    port: None,
                    no_auto_udev: false,
                })?;
            }
            Action::SetActive(slot) => {
                let slot = active_slot_value(slot)?;
                let mut fastboot = wait_for_fastboot().await?;
                fastboot.set_active(slot).await?;
                println!(
                    "{}",
                    simple_status_line(Tone::Success, "slot", &format!("active slot set to {slot}"))
                );
            }
            Action::GetVar(var) => {
                let mut fastboot = wait_for_fastboot().await?;
                println!(
                    "{}",
                    simple_status_line(
                        Tone::Info,
                        "getvar",
                        &format!("{var}: {}", fastboot.get_var(&var).await?)
                    )
                );
            }
            Action::GetVarAll => {
                let mut fastboot = wait_for_fastboot().await?;
                let mut vars = fastboot
                    .get_all_vars()
                    .await?
                    .into_iter()
                    .collect::<Vec<_>>();
                vars.sort_by(|a, b| a.0.cmp(&b.0));
                for (key, value) in vars {
                    println!("{}", simple_status_line(Tone::Info, &key, &value));
                }
            }
            Action::Scatter {
                scatter,
                mode,
                slot,
                include_preloader,
            } => scatter_flow(&scatter, mode, slot, include_preloader, args.yes).await?,
            Action::DisableVbmeta => {
                let empty_vbmeta = standalone_disable_vbmeta_path()?;
                let actions = disable_vbmeta_actions(&empty_vbmeta)?;
                manual_flash_flow(args.dry_run, args.yes, "Disable vbmeta plan", actions).await?;
            }
            Action::Gsi { image } => {
                gsi_flow(args.dry_run, args.yes, &image).await?;
            }
            Action::FormatUserdata { erase_fallback } => {
                format_userdata_flow(args.yes, erase_fallback).await?;
            }
            Action::Flash {
                partition,
                image,
                slot,
            } => {
                let actions = manual_flash_actions(partition, image, slot)?;
                manual_flash_flow(args.dry_run, args.yes, "Manual flash plan", actions).await?;
            }
            Action::Reboot => {
                let mut fastboot = wait_for_fastboot().await?;
                fastboot.reboot().await?;
            }
            Action::RebootBootloader => {
                let mut fastboot = wait_for_fastboot().await?;
                fastboot.reboot_bootloader().await?;
            }
            Action::UnlockBootloader => unlock_bootloader_flow(args.dry_run, args.yes).await?,
            Action::LockBootloader => lock_bootloader_flow(args.dry_run, args.yes).await?,
            Action::WipeData {
                no_metadata,
                no_cache,
                erase_fallback,
            } => wipe_data_flow(args.yes, no_metadata, no_cache, erase_fallback).await?,
        }
    }

    Ok(())
}

fn collect_actions(args: &Args) -> anyhow::Result<Vec<Action>> {
    if let Some(command) = &args.command {
        return Ok(vec![command_action(args, command)?]);
    }

    let mut actions = Vec::new();
    if args.force_fastboot {
        actions.push(Action::ForceFastboot);
    }
    if let Some(slot) = args.set_active {
        actions.push(Action::SetActive(slot));
    }
    if let Some(var) = &args.getvar {
        actions.push(Action::GetVar(var.clone()));
    }
    if args.getvar_all {
        actions.push(Action::GetVarAll);
    }
    if let Some(scatter) = &args.flash {
        actions.push(Action::Scatter {
            scatter: scatter.clone(),
            mode: flash_mode_from_flags(
                args.dry_run,
                args.firmware_upgrade,
                args.clean_flash,
                args.selective,
            )
            .map_err(anyhow::Error::msg)?,
            slot: args.slot,
            include_preloader: args.include_preloader,
        });
    }
    if args.reboot {
        actions.push(Action::Reboot);
    }
    if args.reboot_bootloader {
        actions.push(Action::RebootBootloader);
    }
    Ok(actions)
}

fn command_action(args: &Args, command: &Command) -> anyhow::Result<Action> {
    Ok(match command {
        Command::DisableVbmeta => Action::DisableVbmeta,
        Command::Gsi { image } => Action::Gsi {
            image: image.clone(),
        },
        Command::Format {
            partition: _,
            erase_fallback,
        } => Action::FormatUserdata {
            erase_fallback: *erase_fallback,
        },
        Command::Scatter {
            scatter,
            firmware_upgrade,
            clean_flash,
            selective,
            slot,
            include_preloader,
        } => Action::Scatter {
            scatter: scatter.clone(),
            mode: flash_mode_from_flags(args.dry_run, *firmware_upgrade, *clean_flash, *selective)
                .map_err(anyhow::Error::msg)?,
            slot: *slot,
            include_preloader: *include_preloader,
        },
        Command::Flash {
            partition,
            image,
            slot,
        } => Action::Flash {
            partition: partition.clone(),
            image: image.clone(),
            slot: *slot,
        },
        Command::Reboot => Action::Reboot,
        Command::Getvar { var } => Action::GetVar(var.clone()),
        Command::UnlockBootloader => Action::UnlockBootloader,
        Command::LockBootloader => Action::LockBootloader,
        Command::WipeData {
            no_metadata,
            no_cache,
            erase_fallback,
        } => Action::WipeData {
            no_metadata: *no_metadata,
            no_cache: *no_cache,
            erase_fallback: *erase_fallback,
        },
    })
}

async fn scatter_flow(
    scatter: &Path,
    mode: FlashMode,
    slot: Option<SlotArg>,
    include_preloader: bool,
    yes: bool,
) -> anyhow::Result<()> {
    let dry_run = mode == FlashMode::DryRun;
    let parts = if mode == FlashMode::Selective {
        select_partitions(scatter, slot, include_preloader)?
    } else {
        Vec::new()
    };
    let plan = build_flash_plan(scatter, mode, slot, include_preloader, &parts, !dry_run)?;
    print_plan(&plan);

    if !dry_run && !plan.errors.is_empty() {
        bail!(
            "refusing to flash because the plan has {} error(s)",
            plan.errors.len()
        );
    }

    if dry_run {
        println!("{}", mock_device_info());
        if should_confirm_before_simulation(yes)
            && !Confirm::new("Begin dry-run simulation?")
                .with_default(false)
                .prompt()?
        {
            bail!("aborted by user");
        }
        simulate_plan(&plan)?;
        return Ok(());
    }

    let mut fastboot = wait_for_fastboot().await?;
    let vars = fastboot.get_all_vars().await?;
    println!("{}", compact_device_info(&vars));

    if !yes
        && !Confirm::new("Begin flashing this plan?")
            .with_default(false)
            .prompt()?
    {
        bail!("aborted by user");
    }

    let summary = execute_plan(&plan, &mut fastboot, yes).await?;
    print_completion("Flashing complete", summary);

    if Confirm::new("Reboot device now?")
        .with_default(false)
        .prompt()?
    {
        fastboot.reboot().await?;
    }

    Ok(())
}

async fn manual_flash_flow(
    dry_run: bool,
    yes: bool,
    title: &str,
    actions: Vec<ManualFlashAction>,
) -> anyhow::Result<()> {
    print_manual_plan(title, &actions);

    if dry_run {
        println!("{}", mock_device_info());
        if should_confirm_before_simulation(yes)
            && !Confirm::new("Begin dry-run simulation?")
                .with_default(false)
                .prompt()?
        {
            bail!("aborted by user");
        }
        simulate_manual_actions(&actions)?;
        return Ok(());
    }

    let mut fastboot = wait_for_fastboot().await?;
    let vars = fastboot.get_all_vars().await?;
    println!("{}", compact_device_info(&vars));
    let resolved_actions = actions
        .iter()
        .cloned()
        .map(|mut action| {
            action.partition = action.resolved_partition(&vars)?;
            Ok(action)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    if !yes
        && !Confirm::new("Begin flashing this plan?")
            .with_default(false)
            .prompt()?
    {
        bail!("aborted by user");
    }

    let summary = execute_manual_actions(&resolved_actions, &mut fastboot, yes).await?;
    print_completion("Flashing complete", summary);

    if Confirm::new("Reboot device now?")
        .with_default(false)
        .prompt()?
    {
        fastboot.reboot().await?;
    }

    Ok(())
}

async fn gsi_flow(dry_run: bool, yes: bool, image: &Path) -> anyhow::Result<()> {
    if dry_run {
        bail!("--dry-run is not supported for the gsi subcommand");
    }

    let metadata = std::fs::metadata(image)
        .with_context(|| format!("read GSI image metadata for {}", image.display()))?;
    if !metadata.is_file() {
        bail!("{} is not a regular file", image.display());
    }

    let tools = FormatTools::from_cli_assets()?;
    let mut fastboot = wait_for_fastboot().await?;
    let vars = fastboot.get_all_vars().await?;
    let start_mode = detect_fastboot_mode(&vars);
    println!("{}", compact_device_info(&vars));

    print_destruction_warning(
        "flash gsi",
        "This will flash a GSI, wipe userdata, and reboot between bootloader and fastbootd as needed.",
    );
    println!(
        "{}",
simple_status_line(
            Tone::Info,
            "gsi",
            &format!("system image {}", image.display())
        )
    );
    println!(
        "{}",
simple_status_line(
            Tone::Info,
            "gsi",
            &format!("using bundled formatter root {}", tools.root.display())
        )
    );

    if !yes
        && !Confirm::new("Run the GSI flashing flow on this device?")
            .with_default(false)
            .prompt()?
    {
        bail!("aborted by user");
    }

    let inspected = inspect_gsi_image(image)
        .with_context(|| format!("inspect GSI image {}", image.display()))?;
    let userdata = detect_userdata(&mut fastboot).await?;
    let vbmeta_size =
        std::fs::metadata(fastboot_flasher::manual::resolved_disable_vbmeta_image_path()?)
            .context("read bundled vbmeta metadata")?
            .len();
    let needs_product_gsi =
        maybe_needs_product_gsi(&mut fastboot, &vars, inspected.expanded_size).await?;
    let execution_plan = build_gsi_execution_plan(
        start_mode,
        metadata.len(),
        vbmeta_size,
        &userdata,
        &GsiFlashOptions::default(),
        needs_product_gsi,
    );

    let mut progress = GsiCliProgress::new(ActionSummary {
        flash_count: execution_plan.summary.flash_count,
        wipe_count: execution_plan.summary.wipe_count,
        skipped_count: execution_plan.summary.skipped_count,
        total_bytes: execution_plan.summary.total_bytes,
    });
    let outcome = execute_gsi_flash_with_vars(
        fastboot,
        vars,
        image,
        &tools,
        &GsiFlashOptions::default(),
        |event| match event {
            GsiEvent::Step(step) => {
                progress.println_info("gsi", step.as_str());
            }
            GsiEvent::ModeDetected(mode) => {
                progress.println_info("mode", &format!("device detected in {}", mode.as_str()));
            }
            GsiEvent::ModeReady(mode) => {
                progress.println_info("mode", &format!("device ready in {}", mode.as_str()));
            }
            GsiEvent::UserdataEraseFallback { fs_type } => {
                progress.println_info(
                    "wipe",
                    &format!("userdata type `{fs_type}` uses erase fallback"),
                );
            }
            GsiEvent::ResolvedPartition {
                base,
                partition,
                size_bytes,
            } => {
                let detail = if size_bytes > 0 {
                    format!("{base} -> {partition} (0x{size_bytes:x})")
                } else {
                    format!("{base} -> {partition}")
                };
                progress.println_info("partition", &detail);
            }
            GsiEvent::Flashing {
                partition,
                image,
                size_bytes,
            } => {
                progress.start_flash(
                    &partition,
                    &format!(
                        "flash {} <- {} ({})",
                        partition,
                        image.display(),
                        HumanBytes(size_bytes)
                    ),
                    size_bytes,
                );
            }
            GsiEvent::FlashProgress {
                partition,
                bytes,
                total_bytes,
                speed_bps,
            } => {
                progress.update_flash(&partition, bytes, total_bytes, speed_bps);
            }
            GsiEvent::FlashFinished {
                partition,
                size_bytes,
            } => {
                progress.finish_flash(&partition, size_bytes);
            }
            GsiEvent::Erasing { partition } => {
                progress.increment_overall(1);
                progress.println_info("erase", &format!("best-effort erase {partition}"));
            }
            GsiEvent::EraseFinished { .. } => {}
            GsiEvent::PartitionSkipped { partition, reason } => {
                progress.increment_overall(1);
                progress.println_warn("skip", &format!("{partition}: {reason}"));
            }
        },
    )
    .await?;

    print_completion(
        "GSI flash complete",
        ActionSummary {
            flash_count: outcome.summary.flash_count,
            wipe_count: outcome.summary.wipe_count,
            skipped_count: outcome.summary.skipped_count,
            total_bytes: outcome.summary.total_bytes,
        },
    );

    drop(outcome.device);
    Ok(())
}

struct GsiCliProgress {
    multi: MultiProgress,
    overall: ProgressBar,
    layout: ProgressLayout,
    message_width: usize,
    overall_total: u64,
    current_partition: Option<String>,
    current_bar: Option<ProgressBar>,
}

impl GsiCliProgress {
    fn new(summary: ActionSummary) -> Self {
        let layout = ProgressLayout::from_terminal();
        let message_width = 80;
        let multi = MultiProgress::new();
        let overall = multi.add(ProgressBar::new(summary.total_bytes.max(1)));
        let (overall_prefix, overall_bar_width) = total_row_prefix_and_bar(
            &layout,
            TOTAL_LABEL,
            &format_byte_pair(0, summary.total_bytes.max(1)),
            false,
        );
        overall.set_style(active_total_style(overall_bar_width));
        overall.set_prefix(overall_prefix);
        overall.enable_steady_tick(Duration::from_millis(80));
        let _ = multi.println(progress_header(summary, false));
        Self {
            multi,
            overall,
            layout,
            message_width,
            overall_total: summary.total_bytes.max(1),
            current_partition: None,
            current_bar: None,
        }
    }

    fn println_info(&self, label: &str, detail: &str) {
        let _ = self.multi.println(format!("{label:<10} {detail}"));
    }

    fn println_warn(&self, label: &str, detail: &str) {
        let _ = self.multi.println(format!("{label:<10} {detail}"));
    }

    fn start_flash(&mut self, partition: &str, message: &str, total_bytes: u64) {
        self.finish_open_bar_if_mismatched(partition, total_bytes);
        if self.current_partition.as_deref() == Some(partition) {
            return;
        }

        self.overall_total = self.overall_total.saturating_add(total_bytes.max(1));
        self.overall.set_length(self.overall_total.max(1));

        let byte_pair = format_byte_pair(0, total_bytes.max(1));
        let (prefix, bar_width) = flash_row_prefix_and_bar(
            &self.layout,
            "FLASH",
            "FLASH",
            self.message_width,
            &byte_pair,
        );
        let pb = self
            .multi
            .insert_before(&self.overall, ProgressBar::new(total_bytes.max(1)));
        pb.set_style(active_flash_style(bar_width, self.message_width));
        pb.set_prefix(prefix);
        pb.set_message(message.to_string());

        self.current_partition = Some(partition.to_string());
        self.current_bar = Some(pb);
    }

    fn update_flash(&mut self, partition: &str, bytes: u64, total_bytes: u64, speed_bps: u64) {
        if self.current_partition.as_deref() != Some(partition) {
            self.start_flash(
                partition,
                &format!("flash {partition} ({})", HumanBytes(total_bytes)),
                total_bytes,
            );
        }
        if let Some(pb) = &self.current_bar {
            let current = pb.position();
            if bytes > current {
                let delta = bytes - current;
                pb.inc(delta);
                self.overall.inc(delta);
            }
            if speed_bps > 0 {
                pb.set_message(format!("flash {partition} @ {}/s", HumanBytes(speed_bps)));
            }
        }
    }

    fn finish_flash(&mut self, partition: &str, size_bytes: u64) {
        if self.current_partition.as_deref() != Some(partition) {
            self.start_flash(
                partition,
                &format!("flash {partition} ({})", HumanBytes(size_bytes)),
                size_bytes,
            );
        }
        if let Some(pb) = self.current_bar.take() {
            let remaining = pb
                .length()
                .unwrap_or(size_bytes.max(1))
                .saturating_sub(pb.position());
            if remaining > 0 {
                pb.inc(remaining);
                self.overall.inc(remaining);
            }
            pb.set_style(history_row_style(self.message_width));
            pb.set_prefix(String::new());
            pb.finish_with_message(format!("flash {partition} {}", HumanBytes(size_bytes)));
        }
        self.current_partition = None;
    }

    fn increment_overall(&self, bytes: u64) {
        self.overall.inc(bytes);
    }

    fn finish_open_bar_if_mismatched(&mut self, partition: &str, size_bytes: u64) {
        if self.current_partition.as_deref() != Some(partition) {
            if let Some(current) = self.current_partition.clone() {
                self.finish_flash(&current, size_bytes);
            }
        }
    }
}

async fn unlock_bootloader_flow(dry_run: bool, yes: bool) -> anyhow::Result<()> {
    bootloader_state_flow(dry_run, yes, "unlock").await
}

async fn lock_bootloader_flow(dry_run: bool, yes: bool) -> anyhow::Result<()> {
    bootloader_state_flow(dry_run, yes, "lock").await
}

async fn bootloader_state_flow(dry_run: bool, yes: bool, verb: &str) -> anyhow::Result<()> {
    if dry_run {
        println!("{}", mock_device_info());
        println!(
            "{}",
    simple_status_line(
                Tone::Info,
                "dry-run",
                &format!("would send `flashing {verb}`")
            )
        );
        return Ok(());
    }

    let mut fastboot = wait_for_fastboot().await?;
    let vars = fastboot.get_all_vars().await?;
    println!("{}", compact_device_info(&vars));

    if !yes
        && !Confirm::new(&format!("Send `flashing {verb}` to this device?"))
            .with_default(false)
            .prompt()?
    {
        bail!("aborted by user");
    }

    match verb {
        "unlock" => fastboot.unlock_bootloader().await?,
        "lock" => fastboot.lock_bootloader().await?,
        other => bail!("unsupported bootloader action: {other}"),
    }

    println!(
        "{}",
simple_status_line(
            Tone::Success,
            "bootloader",
            &format!("sent `flashing {verb}`")
        )
    );

    Ok(())
}

async fn format_userdata_flow(yes: bool, erase_fallback: bool) -> anyhow::Result<()> {
    let tools = FormatTools::from_cli_assets()?;
    let mut fastboot = wait_for_fastboot().await?;
    let vars = fastboot.get_all_vars().await?;
    println!("{}", compact_device_info(&vars));

    let info = detect_userdata(&mut fastboot).await?;
    print_userdata_info(&info);
    print_destruction_warning(
        "format userdata",
        "This will permanently erase /data and rebuild the userdata filesystem.",
    );

    if !yes
        && !Confirm::new("Format userdata on this device?")
            .with_default(false)
            .prompt()?
    {
        bail!("aborted by user");
    }

    println!(
        "{}",
simple_status_line(
            Tone::Info,
            "format",
            &format!("using bundled formatter root {}", tools.root.display())
        )
    );
    println!(
        "{}",
simple_status_line(
            Tone::Info,
            "format",
            &format!("generating {} userdata image", info.fs_type)
        )
    );

    let outcome = format_userdata_with_info(
        &mut fastboot,
        &tools,
        info,
        &FormatUserdataOptions {
            erase_fallback,
            casefold: false,
        },
        |_| {},
    )
    .await?;

    if outcome.used_erase_fallback {
        println!(
            "{}",
    simple_status_line(
                Tone::Warning,
                "format",
                "formatter failed; used erase fallback"
            )
        );
    } else {
        println!(
            "{}",
    simple_status_line(Tone::Success, "done", "userdata format completed")
        );
    }

    Ok(())
}

async fn wipe_data_flow(
    yes: bool,
    no_metadata: bool,
    no_cache: bool,
    erase_fallback: bool,
) -> anyhow::Result<()> {
    let tools = FormatTools::from_cli_assets()?;
    let mut fastboot = wait_for_fastboot().await?;
    let vars = fastboot.get_all_vars().await?;
    println!("{}", compact_device_info(&vars));

    let info = detect_userdata(&mut fastboot).await?;
    print_userdata_info(&info);
    print_destruction_warning(
        "wipe data",
        "This will permanently erase userdata and may erase metadata/cache if present.",
    );

    if !yes
        && !Confirm::new("Wipe data on this device?")
            .with_default(false)
            .prompt()?
    {
        bail!("aborted by user");
    }

    println!(
        "{}",
simple_status_line(
            Tone::Info,
            "format",
            &format!("using bundled formatter root {}", tools.root.display())
        )
    );
    println!(
        "{}",
simple_status_line(
            Tone::Info,
            "format",
            &format!("generating {} userdata image", info.fs_type)
        )
    );

    let outcome = wipe_data_with_info(
        &mut fastboot,
        &tools,
        info,
        &WipeDataOptions {
            erase_metadata: !no_metadata,
            erase_cache: !no_cache,
            erase_fallback,
            casefold: false,
        },
        |_| {},
    )
    .await?;

    if outcome.format.used_erase_fallback {
        println!(
            "{}",
    simple_status_line(
                Tone::Warning,
                "format",
                "formatter failed; used erase fallback"
            )
        );
    }
    println!(
        "{}",
simple_status_line(
            if !no_metadata && outcome.metadata_erased {
                Tone::Success
            } else if no_metadata {
                Tone::Info
            } else {
                Tone::Warning
            },
            "wipe",
            if no_metadata {
                "metadata erase disabled"
            } else if outcome.metadata_erased {
                "metadata erased"
            } else {
                "metadata erase skipped"
            }
        )
    );
    println!(
        "{}",
simple_status_line(
            if !no_cache && outcome.cache_erased {
                Tone::Success
            } else if no_cache {
                Tone::Info
            } else {
                Tone::Warning
            },
            "wipe",
            if no_cache {
                "cache erase disabled"
            } else if outcome.cache_erased {
                "cache erased"
            } else {
                "cache erase skipped"
            }
        )
    );
    println!(
        "{}",
simple_status_line(Tone::Success, "done", "data wipe completed")
    );

    Ok(())
}

fn print_userdata_info(info: &fastboot_flasher::format::UserdataInfo) {
    println!(
        "{}",
simple_status_line(
            Tone::Info,
            "fastboot",
            &format!("partition-type:userdata = {}", info.fs_type)
        )
    );
    println!(
        "{}",
simple_status_line(
            Tone::Info,
            "fastboot",
            &format!("partition-size:userdata = 0x{:x}", info.size)
        )
    );
    if let Some(max) = info.max_download_size {
        println!(
            "{}",
    simple_status_line(
                Tone::Info,
                "fastboot",
                &format!("max-download-size = 0x{max:x}")
            )
        );
    }
}

fn print_destruction_warning(title: &str, detail: &str) {
    eprintln!("{}", simple_notice_box(Tone::Warning, title, detail));
}

fn select_partitions(
    scatter: &Path,
    slot: Option<SlotArg>,
    include_preloader: bool,
) -> anyhow::Result<Vec<String>> {
    let plan = build_flash_plan(
        scatter,
        FlashMode::DryRun,
        slot,
        include_preloader,
        &[],
        false,
    )?;
    let options = plan
        .actions
        .iter()
        .filter(|action| action.action == "flash")
        .map(|action| {
            selective_option_label(&action.partition, &action.safety_class, &action.size_human)
        })
        .collect::<Vec<_>>();
    if options.is_empty() {
        bail!("no selectable flash actions found");
    }
    let selected = MultiSelect::new("Select partitions to flash", options)
        .with_page_size(15)
        .prompt()?;
    let mut parts = Vec::new();
    for label in selected {
        if let Some((partition, _)) = label.split_once(' ') {
            parts.push(partition.to_string());
        }
    }
    if parts.is_empty() {
        bail!("no partitions selected");
    }
    Ok(parts)
}

fn print_plan(plan: &FlashPlan) {
    println!();
    println!("{}", simple_banner("PLAN SUMMARY"));
    println!();
    let summary_pairs = vec![
        ("Mode", plan.mode.to_string()),
        ("Storage", plan.storage_selection.to_string()),
        ("Slot", plan.slot_policy_effective.to_string()),
        ("Flash", plan.summary.flash_count.to_string()),
        ("Wipe", plan.summary.wipe_count.to_string()),
        ("Skipped", plan.summary.skipped_count.to_string()),
        ("Warnings", (plan.summary.warning_count + plan.summary.action_warning_count).to_string()),
        ("Errors", plan.summary.error_count.to_string()),
    ];
    println!("{}", simple_kv_table(&summary_pairs));
    println!();
    println!("{}", simple_section_header("FLASH PLAN"));
    println!();
    for (index, action) in plan.actions.iter().enumerate() {
        println!(
            "{}. {} {} ({}) [{}] - {}",
            index + 1,
            action.action,
            action.partition,
            action.size_human,
            action.safety_class,
            action.reason
        );
    }
    println!();
    for warning in plan.warnings.iter().take(10) {
        eprintln!(
            "{}",
            simple_notice_box(Tone::Warning, "plan warning", warning.as_str())
        );
    }
    for error in plan.errors.iter().take(10) {
        eprintln!("{}", simple_notice_box(Tone::Error, "plan error", error.as_str()));
    }
}

fn print_manual_plan(title: &str, actions: &[ManualFlashAction]) {
    println!();
    println!("{}", simple_banner(title));
    println!();
    let total_bytes = actions.iter().map(|action| action.size).sum::<u64>();
    let summary_pairs = vec![
        ("Mode", "manual".to_string()),
        ("Flash", actions.len().to_string()),
        ("Wipe", "0".to_string()),
        ("Skipped", "0".to_string()),
        ("Bytes", HumanBytes(total_bytes).to_string()),
    ];
    println!("{}", simple_kv_table(&summary_pairs));
    println!();
    println!("{}", simple_section_header("FLASH PLAN"));
    println!();
    for (index, action) in actions.iter().enumerate() {
        println!(
            "{}. flash {} ({}) - {}",
            index + 1,
            action.partition,
            HumanBytes(action.size),
            action.image.display()
        );
    }
    println!();
}

fn simulate_plan(plan: &FlashPlan) -> anyhow::Result<()> {
    let started = Instant::now();
    let summary = plan_action_summary(plan);
    let layout = ProgressLayout::from_terminal();
    let item_message_width = plan_item_message_width(plan);
    let label_sample =
        active_action_label(plan.actions.len().saturating_sub(1), plan.actions.len());
    let multi = MultiProgress::new();
    let overall = multi.add(ProgressBar::new(summary.total_bytes.max(1)));
    let overall_byte_pair = format_byte_pair(0, summary.total_bytes.max(1));
    let (overall_prefix, overall_bar_width) =
        total_row_prefix_and_bar(&layout, TOTAL_LABEL, &overall_byte_pair, false);
    overall.set_style(active_total_style(overall_bar_width));
    overall.set_prefix(overall_prefix);
    multi.println(progress_header(summary, true))?;
    for (index, action) in plan.actions.iter().enumerate() {
        let total = u64::try_from(action.size).unwrap_or(0).max(1);
        let label = active_action_label(index, plan.actions.len());
        let message = format!("{} {}", action.action, action.partition);
        let byte_pair = format_byte_pair(0, total);
        let (prefix, bar_width) = flash_row_prefix_and_bar(
            &layout,
            &label,
            &label_sample,
            item_message_width,
            &byte_pair,
        );
        let pb = multi.insert_before(&overall, ProgressBar::new(total));
        pb.set_style(active_byte_style(bar_width, item_message_width));
        pb.set_prefix(prefix);
        pb.set_message(message);
        for step in dry_run_steps(total, DRY_RUN_SPEED_MIB) {
            pb.inc(step.bytes);
            overall.inc(step.bytes);
            std::thread::sleep(Duration::from_millis(100));
        }
        let completed_message = match action.action.as_str() {
            "flash" => flash_history_message(
                index,
                plan.actions.len(),
                &action.partition,
                total,
                item_message_width,
            ),
            "wipe" => erase_history_message(index, plan.actions.len(), &action.partition),
            other => format!(
                "{}/{} {other} {}",
                index + 1,
                plan.actions.len(),
                action.partition
            ),
        };
        pb.set_style(history_row_style(item_message_width));
        pb.set_prefix(String::new());
        pb.finish_with_message(completed_message);
    }
    overall.set_style(completed_total_style(overall_bar_width));
    overall.finish();
    print_completion_with_elapsed("Dry-run complete", summary, started.elapsed());
    Ok(())
}

fn simulate_manual_actions(actions: &[ManualFlashAction]) -> anyhow::Result<()> {
    let started = Instant::now();
    let summary = manual_action_summary(actions);
    let layout = ProgressLayout::from_terminal();
    let item_message_width = manual_item_message_width(actions);
    let label_sample = active_action_label(actions.len().saturating_sub(1), actions.len());
    let multi = MultiProgress::new();
    let overall = multi.add(ProgressBar::new(summary.total_bytes.max(1)));
    let overall_byte_pair = format_byte_pair(0, summary.total_bytes.max(1));
    let (overall_prefix, overall_bar_width) =
        total_row_prefix_and_bar(&layout, TOTAL_LABEL, &overall_byte_pair, false);
    overall.set_style(active_total_style(overall_bar_width));
    overall.set_prefix(overall_prefix);
    multi.println(progress_header(summary, true))?;
    for (index, action) in actions.iter().enumerate() {
        let total = action.size.max(1);
        let label = active_action_label(index, actions.len());
        let message = format!("flash {}", action.partition);
        let byte_pair = format_byte_pair(0, total);
        let (prefix, bar_width) = flash_row_prefix_and_bar(
            &layout,
            &label,
            &label_sample,
            item_message_width,
            &byte_pair,
        );
        let pb = multi.insert_before(&overall, ProgressBar::new(total));
        pb.set_style(active_byte_style(bar_width, item_message_width));
        pb.set_prefix(prefix);
        pb.set_message(message);
        for step in dry_run_steps(total, DRY_RUN_SPEED_MIB) {
            pb.inc(step.bytes);
            overall.inc(step.bytes);
            std::thread::sleep(Duration::from_millis(100));
        }
        let completed_message = flash_history_message(
            index,
            actions.len(),
            &action.partition,
            total,
            item_message_width,
        );
        pb.set_style(history_row_style(item_message_width));
        pb.set_prefix(String::new());
        pb.finish_with_message(completed_message);
    }
    overall.set_style(completed_total_style(overall_bar_width));
    overall.finish();
    print_completion_with_elapsed("Dry-run complete", summary, started.elapsed());
    Ok(())
}

async fn execute_plan(
    plan: &FlashPlan,
    fastboot: &mut FastbootDevice,
    yes: bool,
) -> anyhow::Result<ActionSummary> {
    let max_download = fastboot.max_download_size().await?;
    let mut summary = plan_action_summary(plan);
    let layout = ProgressLayout::from_terminal();
    let item_message_width = plan_item_message_width(plan);
    let label_sample =
        active_action_label(plan.actions.len().saturating_sub(1), plan.actions.len());
    let multi = MultiProgress::new();
    let overall = multi.add(ProgressBar::new(summary.total_bytes.max(1)));
    let overall_byte_pair = format_byte_pair(0, summary.total_bytes.max(1));
    let (overall_prefix, overall_bar_width) =
        total_row_prefix_and_bar(&layout, TOTAL_LABEL, &overall_byte_pair, false);
    overall.set_style(active_total_style(overall_bar_width));
    overall.set_prefix(overall_prefix);
    let ctx = FlashContext {
        multi: &multi,
        overall: &overall,
        layout,
        total_count: plan.actions.len(),
        label_sample: label_sample.clone(),
        item_message_width,
    };
    multi.println(progress_header(summary, false))?;
    overall.enable_steady_tick(Duration::from_millis(80));

    for (index, action) in plan.actions.iter().enumerate() {
        match action.action.as_str() {
            "flash" => {
                let image_path = action
                    .image_resolved_path()
                    .map(PathBuf::from)
                    .with_context(|| format!("missing image path for {}", action.partition))?;
                let skipped = execute_one_flash(
                    fastboot,
                    &action.partition,
                    &image_path,
                    index,
                    max_download,
                    yes,
                    &ctx,
                )
                .await?;
                if skipped {
                    summary.skipped_count += 1;
                }
            }
            "wipe" => {
                let pb = insert_action_spinner(&multi, &overall);
                pb.set_style(spinner_row_style(item_message_width));
                let label = active_action_label(index, plan.actions.len());
                let message = format!("erase {}", action.partition);
                pb.set_prefix(spinner_row_prefix(
                    &layout,
                    &label,
                    &label_sample,
                    item_message_width,
                ));
                pb.set_message(message);
                pb.enable_steady_tick(Duration::from_millis(80));
                if let Err(err) = fastboot.erase(&action.partition).await {
                    if handle_failed_erase(yes, &action.partition, &err)? {
                        summary.skipped_count += 1;
                        overall.inc(u64::try_from(action.size).unwrap_or(0));
                        pb.set_prefix(spinner_row_prefix(
                            &layout,
                            &label,
                            &label_sample,
                            item_message_width,
                        ));
                        pb.finish_with_message(skipped_erase_history_message(
                            index,
                            plan.actions.len(),
                            &action.partition,
                        ));
                        continue;
                    }
                    return Err(err).with_context(|| format!("erase {}", action.partition));
                }
                overall.inc(u64::try_from(action.size).unwrap_or(0));
                pb.set_style(history_row_style(item_message_width));
                pb.set_prefix(String::new());
                pb.finish_with_message(erase_history_message(
                    index,
                    plan.actions.len(),
                    &action.partition,
                ));
            }
            other => bail!("unsupported plan action: {other}"),
        }
    }
    overall.set_style(completed_total_style(overall_bar_width));
    overall.finish();
    Ok(summary)
}

async fn execute_manual_actions(
    actions: &[ManualFlashAction],
    fastboot: &mut FastbootDevice,
    yes: bool,
) -> anyhow::Result<ActionSummary> {
    let max_download = fastboot.max_download_size().await?;
    let mut summary = manual_action_summary(actions);
    let layout = ProgressLayout::from_terminal();
    let item_message_width = manual_item_message_width(actions);
    let label_sample = active_action_label(actions.len().saturating_sub(1), actions.len());
    let multi = MultiProgress::new();
    let overall = multi.add(ProgressBar::new(summary.total_bytes.max(1)));
    let overall_byte_pair = format_byte_pair(0, summary.total_bytes.max(1));
    let (overall_prefix, overall_bar_width) =
        total_row_prefix_and_bar(&layout, TOTAL_LABEL, &overall_byte_pair, false);
    overall.set_style(active_total_style(overall_bar_width));
    overall.set_prefix(overall_prefix);
    let ctx = FlashContext {
        multi: &multi,
        overall: &overall,
        layout,
        total_count: actions.len(),
        label_sample: label_sample.clone(),
        item_message_width,
    };
    multi.println(progress_header(summary, false))?;
    overall.enable_steady_tick(Duration::from_millis(80));

    for (index, action) in actions.iter().enumerate() {
        let skipped = execute_one_flash(
            fastboot,
            &action.partition,
            &action.image,
            index,
            max_download,
            yes,
            &ctx,
        )
        .await?;
        if skipped {
            summary.skipped_count += 1;
        }
    }
    overall.set_style(completed_total_style(overall_bar_width));
    overall.finish();
    Ok(summary)
}

struct FlashContext<'a> {
    multi: &'a MultiProgress,
    overall: &'a ProgressBar,
    layout: ProgressLayout,
    total_count: usize,
    label_sample: String,
    item_message_width: usize,
}

async fn execute_one_flash(
    fastboot: &mut FastbootDevice,
    partition: &str,
    image_path: &Path,
    index: usize,
    max_download: u32,
    yes: bool,
    ctx: &FlashContext<'_>,
) -> anyhow::Result<bool> {
    let prepared = prepare_image(image_path, max_download)
        .with_context(|| format!("prepare image for {partition}"))?;
    let total_download_bytes = prepared
        .transfers
        .iter()
        .map(|transfer| u64::from(transfer.download_size()))
        .sum::<u64>()
        .max(1);
    let pb = ctx
        .multi
        .insert_before(ctx.overall, ProgressBar::new(total_download_bytes));
    let label = active_action_label(index, ctx.total_count);
    let message = format!("flash {partition}");
    let byte_pair = format_byte_pair(0, total_download_bytes);
    let (prefix, bar_width) = flash_row_prefix_and_bar(
        &ctx.layout,
        &label,
        &ctx.label_sample,
        ctx.item_message_width,
        &byte_pair,
    );
    pb.set_style(active_flash_style(bar_width, ctx.item_message_width));
    pb.set_prefix(prefix);
    pb.set_message(message);
    let result = flash_prepared_image(fastboot, partition, &prepared, |event| {
        update_flash_progress(&pb, ctx.overall, event)
    })
    .await;
    match result {
        Ok(()) => {
            let completed_message = flash_history_message(
                index,
                ctx.total_count,
                partition,
                total_download_bytes,
                ctx.item_message_width,
            );
            pb.set_style(history_row_style(ctx.item_message_width));
            pb.set_prefix(String::new());
            pb.finish_with_message(completed_message);
            Ok(false)
        }
        Err(err) => {
            if handle_failed_partition(yes, partition, &err)? {
                pb.set_style(history_row_style(ctx.item_message_width));
                pb.set_prefix(String::new());
                pb.finish_with_message(skipped_flash_history_message(
                    index,
                    ctx.total_count,
                    partition,
                    total_download_bytes,
                    ctx.item_message_width,
                ));
                return Ok(true);
            }
            Err(err).with_context(|| format!("flash {partition}"))
        }
    }
}

fn update_flash_progress(pb: &ProgressBar, overall: &ProgressBar, event: FlashProgress) {
    match event {
        FlashProgress::DownloadStarted { .. } => {}
        FlashProgress::DownloadFinished { .. } => {}
        FlashProgress::DownloadBytes { bytes, .. } => {
            pb.inc(bytes);
            overall.inc(bytes);
        }
        FlashProgress::FlashStarted { .. } => {}
        FlashProgress::FlashFinished { .. } => {}
    }
}

async fn wait_for_fastboot() -> anyhow::Result<FastbootDevice> {
    let spinner = ProgressBar::new_spinner();
    spinner.set_message("waiting for fastboot device");
    spinner.enable_steady_tick(Duration::from_millis(120));
    loop {
        match connect_fastboot().await {
            Ok(dev) => {
                spinner.finish_and_clear();
                return Ok(dev);
            }
            Err(_) => sleep(Duration::from_millis(250)).await,
        }
    }
}

fn active_slot_value(slot: SlotArg) -> anyhow::Result<&'static str> {
    match slot {
        SlotArg::A => Ok("a"),
        SlotArg::B => Ok("b"),
        SlotArg::Active | SlotArg::Inactive | SlotArg::All => {
            bail!("--set-active only accepts a or b")
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ProgressLayout {
    terminal_width: usize,
}

impl ProgressLayout {
    fn from_terminal() -> Self {
        let columns = crossterm::terminal::size()
            .map_or(100, |(columns, _)| columns);
        Self {
            terminal_width: usize::from(columns),
        }
    }
}

const SPINNER_SAMPLE: &str = "▹▹▹▹▹";
const BAR_PLACEHOLDER_WIDTH: usize = 2;
const BAR_MIN_WIDTH: usize = 10;
const BAR_MAX_WIDTH: usize = 28;
const TOTAL_LABEL: &str = "TOTAL";

fn row_bar_width(layout: &ProgressLayout, sample_without_bar: &str) -> usize {
    let available = layout
        .terminal_width
        .saturating_sub(visible_width(sample_without_bar).saturating_sub(BAR_PLACEHOLDER_WIDTH));
    fit_width(available, BAR_MIN_WIDTH, BAR_MAX_WIDTH)
}

fn centered_row_prefix(
    _layout: &ProgressLayout,
    label: &str,
    _sample_without_bar: &str,
    _bar_width: usize,
) -> String {
    label.to_string()
}

fn active_byte_style(bar_width: usize, message_width: usize) -> ProgressStyle {
    timed_style(&format!(
        "{{prefix}} {{spinner:.green}} [{{bar:{bar_width}.cyan/blue}}] {{byte_pair}} {{msg:<{message_width}}}"
    ))
}

fn active_total_template(bar_width: usize) -> String {
    format!(
        "{{prefix}} {{spinner:.green}} [{{bar:{bar_width}.blue/black}}] {{byte_pair}} eta {{eta_mmss}}"
    )
}

fn active_total_style(bar_width: usize) -> ProgressStyle {
    timed_style(&active_total_template(bar_width))
}

fn completed_total_template(bar_width: usize) -> String {
    format!("{{prefix}} {{spinner:.green}} [{{bar:{bar_width}.green/green}}] {{byte_pair}}")
}

fn completed_total_style(bar_width: usize) -> ProgressStyle {
    timed_style(&completed_total_template(bar_width))
}

fn active_flash_style(bar_width: usize, message_width: usize) -> ProgressStyle {
    timed_style(&format!(
        "{{prefix}} {{spinner:.green}} [{{bar:{bar_width}.cyan/blue}}] {{byte_pair}} {{msg:<{message_width}}}"
    ))
}

fn history_row_style(message_width: usize) -> ProgressStyle {
    timed_style(&format!("{{msg:<{message_width}}}"))
}

fn spinner_row_style(message_width: usize) -> ProgressStyle {
    timed_style(&format!(
        "{{prefix}} {{spinner:.green}} {{msg:<{message_width}}}"
    ))
}

fn flash_row_prefix_and_bar(
    layout: &ProgressLayout,
    prefix_label: &str,
    sample_label: &str,
    message_width: usize,
    byte_pair: &str,
) -> (String, usize) {
    let sample_without_bar = format!(
        "{sample_label} {SPINNER_SAMPLE} [] {byte_pair} {}",
        "x".repeat(message_width.max(1))
    );
    let bar_width = row_bar_width(layout, &sample_without_bar);
    let prefix = centered_row_prefix(layout, prefix_label, &sample_without_bar, bar_width);
    (prefix, bar_width)
}

fn spinner_row_prefix(
    _layout: &ProgressLayout,
    prefix_label: &str,
    _sample_label: &str,
    _message_width: usize,
) -> String {
    prefix_label.to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionRowPlacement {
    BeforeTotal,
}

fn action_row_placement() -> ActionRowPlacement {
    ActionRowPlacement::BeforeTotal
}

fn insert_action_spinner(multi: &MultiProgress, overall: &ProgressBar) -> ProgressBar {
    match action_row_placement() {
        ActionRowPlacement::BeforeTotal => multi.insert_before(overall, ProgressBar::new_spinner()),
    }
}

fn total_row_prefix_and_bar(
    layout: &ProgressLayout,
    prefix_label: &str,
    byte_pair: &str,
    completed: bool,
) -> (String, usize) {
    let sample_without_bar = if completed {
        format!("{prefix_label} {SPINNER_SAMPLE} [] {byte_pair}")
    } else {
        format!("{prefix_label} {SPINNER_SAMPLE} [] {byte_pair} eta 00:00")
    };
    let bar_width = row_bar_width(layout, &sample_without_bar);
    let prefix = centered_row_prefix(layout, prefix_label, &sample_without_bar, bar_width);
    (prefix, bar_width)
}

fn timed_style(template: &str) -> ProgressStyle {
    let fallback_template = "{spinner:.green} [{bar:10.cyan/blue}] {bytes}/{total}";
    ProgressStyle::with_template(template)
        .unwrap_or_else(|_| {
            eprintln!("[progress] invalid template, using fallback");
            ProgressStyle::with_template(fallback_template)
                .expect("static fallback template is always valid")
        })
        .with_key(
            "elapsed_mmss",
            |state: &ProgressState, out: &mut dyn std::fmt::Write| {
                let _ = write!(out, "{}", format_mm_ss(state.elapsed()));
            },
        )
        .with_key(
            "eta_mmss",
            |state: &ProgressState, out: &mut dyn std::fmt::Write| {
                let _ = write!(out, "{}", format_mm_ss(state.eta()));
            },
        )
        .with_key(
            "byte_pair",
            |state: &ProgressState, out: &mut dyn std::fmt::Write| {
                let total = state.len().unwrap_or_else(|| state.pos());
                let _ = write!(out, "{}", format_byte_pair(state.pos(), total));
            },
        )
        .progress_chars("=> ")
        .tick_strings(&[
            "▹▹▹▹▹",
            "▸▹▹▹▹",
            "▹▸▹▹▹",
            "▹▹▸▹▹",
            "▹▹▹▸▹",
            "▹▹▹▹▸",
            "▪▪▪▪▪",
        ])
}

fn plan_action_summary(plan: &FlashPlan) -> ActionSummary {
    action_summary(plan.actions.iter().map(|action| {
        debug_assert!(action.size >= 0, "flash plan size must be non-negative");
        (
            action.action.as_str(),
            u64::try_from(action.size).unwrap_or(0),
        )
    }))
}

fn plan_item_message_width(plan: &FlashPlan) -> usize {
    max_visible_width(plan.actions.iter().flat_map(|action| {
        let size = u64::try_from(action.size).unwrap_or(0);
        match action.action.as_str() {
            "flash" => [
                format!("{} {}", action.action, action.partition),
                flash_history_message(
                    0,
                    plan.actions.len(),
                    &action.partition,
                    size,
                    flash_history_min_width(0, plan.actions.len(), &action.partition, size),
                ),
                skipped_flash_history_message(
                    0,
                    plan.actions.len(),
                    &action.partition,
                    size,
                    skipped_flash_history_min_width(0, plan.actions.len(), &action.partition, size),
                ),
            ],
            "wipe" => [
                format!("{} {}", action.action, action.partition),
                erase_history_message(0, plan.actions.len(), &action.partition),
                skipped_erase_history_message(0, plan.actions.len(), &action.partition),
            ],
            _ => [
                format!("{} {}", action.action, action.partition),
                format!("{} {}", action.action, action.partition),
                format!("skipped {} {}", action.action, action.partition),
            ],
        }
    }))
}

fn manual_item_message_width(actions: &[ManualFlashAction]) -> usize {
    max_visible_width(actions.iter().flat_map(|action| {
        [
            format!("flash {}", action.partition),
            flash_history_message(
                0,
                actions.len(),
                &action.partition,
                action.size,
                flash_history_min_width(0, actions.len(), &action.partition, action.size),
            ),
            skipped_flash_history_message(
                0,
                actions.len(),
                &action.partition,
                action.size,
                skipped_flash_history_min_width(0, actions.len(), &action.partition, action.size),
            ),
        ]
    }))
}

fn manual_action_summary(actions: &[ManualFlashAction]) -> ActionSummary {
    action_summary(actions.iter().map(|action| ("flash", action.size)))
}

fn print_completion(title: &str, summary: ActionSummary) {
    println!();
    println!("{}", simple_banner(title));
    println!();
    let pairs = vec![
        ("Actions", summary.action_count().to_string()),
        ("Flash", summary.flash_count.to_string()),
        ("Wipe", summary.wipe_count.to_string()),
        ("Skipped", summary.skipped_count.to_string()),
        ("Bytes", HumanBytes(summary.total_bytes).to_string()),
    ];
    println!("{}", simple_kv_table(&pairs));
}

fn print_completion_with_elapsed(title: &str, summary: ActionSummary, elapsed: Duration) {
    print_completion(title, summary);
    println!();
    println!(
        "{}",
simple_status_line(Tone::Accent, "elapsed", &format_mm_ss(elapsed))
    );
}

fn _mode_for_docs(mode: FlashMode) -> mtk_scatter_parser::Mode {
    mode_to_scatter(mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastboot_rs::FlashProgress;
    use indicatif::ProgressBar;

    #[test]
    fn active_total_row_should_not_reserve_elapsed_message_space() {
        let (prefix, bar_width) = total_row_prefix_and_bar(
            &ProgressLayout {
                terminal_width: 100,
            },
            TOTAL_LABEL,
            &format_byte_pair(0, 12_430_000_000),
            false,
        );

        assert_eq!(prefix, TOTAL_LABEL);
        assert!(bar_width >= BAR_MIN_WIDTH);
    }

    #[test]
    fn active_total_template_should_render_eta_without_elapsed_timer() {
        let template = active_total_template(12);

        assert!(template.contains("eta {eta_mmss}"));
        assert!(!template.contains("elapsed_mmss"));
        assert!(!template.contains("{msg}"));
    }

    #[test]
    fn completed_total_template_should_drop_eta_field() {
        let template = completed_total_template(12);

        assert!(!template.contains("eta"));
        assert!(!template.contains("elapsed_mmss"));
    }

    #[test]
    fn update_flash_progress_should_advance_partition_and_total_bytes() {
        let partition = ProgressBar::new(1024);
        let total = ProgressBar::new(4096);

        update_flash_progress(
            &partition,
            &total,
            FlashProgress::DownloadBytes {
                transfer_index: 1,
                transfer_count: 2,
                bytes: 512,
            },
        );

        assert_eq!(partition.position(), 512);
        assert_eq!(total.position(), 512);
    }

    #[test]
    fn action_rows_should_insert_before_total() {
        assert_eq!(action_row_placement(), ActionRowPlacement::BeforeTotal);
    }
}
