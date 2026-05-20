use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use fastboot_rs::{FastbootDevice, FlashProgress};
use mtk_scatter_parser::{FlashAction, FlashActionExecutionKind};

use tracing::warn;

use crate::{
    device::resolve_flash_partition_target,
    device::{read_all_variables, reboot_device, resolve_max_download_size_from_vars},
    flash::{erase_one_partition, flash_one_partition, is_scatter_skippable_error},
    format::{
        detect_userdata, erase_optional_partition, generate_userdata_image, FormatTools,
        FormatUserdataOptions, OptionalEraseOutcome, UserdataInfo, WipeDataOptions,
    },
    manual::ManualFlashAction,
};

use crate::domain::{
    filter_actions, total_bytes_for_actions, update_overall_progress, FlashEvent, FlashOperation,
    FlashRunControl, FlashSummaryDto,
};

/// Outcome of flashing a single partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionFlashOutcome {
    /// The partition action completed successfully.
    Completed,
    /// The partition action was skipped after a non-fatal failure.
    Skipped,
}

/// Whether a partition flash failure can be skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionFlashFailureDisposition {
    /// The failure can be skipped and the run may continue.
    Skip,
    /// The failure is fatal and should stop the run.
    Fatal,
}

/// Classify whether a partition flash error should stop execution.
pub fn partition_flash_failure_disposition(
    error: &anyhow::Error,
) -> PartitionFlashFailureDisposition {
    if is_scatter_skippable_error(error) {
        PartitionFlashFailureDisposition::Skip
    } else {
        PartitionFlashFailureDisposition::Fatal
    }
}

fn action_is_skip_eligible(action: &FlashAction) -> bool {
    !matches!(
        action.safety_class.as_str(),
        "bootloader_critical" | "boot_critical" | "android_system"
    )
}

fn wipe_failure_is_skip_eligible(action: &FlashAction, error: &anyhow::Error) -> bool {
    action.execution_kind == FlashActionExecutionKind::EraseOptional
        && action_is_skip_eligible(action)
        && is_scatter_skippable_error(error)
}

/// Progress context for flash operations that emit shared events.
pub struct FlashProgressContext<'a, E>
where
    E: FnMut(FlashEvent) -> Result<(), String>,
{
    /// Connected fastboot device used for flash operations.
    pub dev: &'a mut FastbootDevice,
    /// Event sink shared by CLI and GUI adapters.
    pub emit: E,
    /// Mutable run summary to update as actions complete.
    pub summary: &'a mut FlashSummaryDto,
    /// Cancellation token for the current run.
    pub control: &'a FlashRunControl,
    /// Device-reported maximum download size.
    pub max_download_size: u32,
    /// Total number of bytes represented by the full run.
    pub overall_total: u64,
}

/// Options for executing a scatter flash plan on a connected device.
pub struct ScatterFlashOptions<'a> {
    /// Selected partition filters. Empty means all plan actions.
    pub partitions: &'a [String],
    /// Per-partition image path overrides.
    pub image_overrides: &'a HashMap<String, String>,
    /// Whether to emit a `PlanBuilt` event before execution.
    pub announce_plan: bool,
    /// Whether to reboot to system after flashing completes.
    pub reboot: bool,
    /// Bundled filesystem-formatting tools used for userdata clean-flash actions.
    pub format_tools: Option<&'a FormatTools>,
    /// Cancellation token for the current run.
    pub control: &'a FlashRunControl,
}

/// Mutable execution context for a manual flash sequence.
pub struct ManualActionExecution<'a> {
    /// Device-reported maximum download size.
    pub max_download_size: u32,
    /// Resolves the fastboot partition to use for each manual action.
    pub partition_resolver: &'a dyn Fn(&str) -> String,
    /// Cancellation token for the current run.
    pub control: &'a FlashRunControl,
    /// Mutable summary updated as each action completes.
    pub summary: &'a mut FlashSummaryDto,
    /// Total bytes represented by the manual action set.
    pub overall_total: u64,
}

impl<'a, E> FlashProgressContext<'a, E>
where
    E: FnMut(FlashEvent) -> Result<(), String>,
{
    /// Flash a partition and emit shared progress events.
    pub async fn flash_partition(
        &mut self,
        partition: &str,
        image_path: &Path,
        bytes: u64,
        completed_before: u64,
        allow_skip_failed_partition: bool,
        operation: FlashOperation,
    ) -> Result<PartitionFlashOutcome, String> {
        self.control.ensure_not_cancelled()?;
        (self.emit)(FlashEvent::PreparingImage {
            partition: partition.to_string(),
            operation,
        })?;

        emit_overall_progress(&mut self.emit, completed_before, 0, self.overall_total)?;

        let result = self
            .flash_one_partition_evented(partition, image_path, bytes, completed_before, operation)
            .await;

        match result {
            Ok(()) => {
                match operation {
                    FlashOperation::Flash => self.summary.flash_count += 1,
                    FlashOperation::FormatUserdata | FlashOperation::Erase => {
                        self.summary.wipe_count += 1
                    }
                }
                emit_overall_progress(&mut self.emit, completed_before, bytes, self.overall_total)?;
                (self.emit)(FlashEvent::PartitionComplete {
                    partition: partition.to_string(),
                    operation,
                })?;
                Ok(PartitionFlashOutcome::Completed)
            }
            Err(error) => match (
                allow_skip_failed_partition,
                partition_flash_failure_disposition(&error),
            ) {
                (true, PartitionFlashFailureDisposition::Skip) => {
                    let reason = format!("{error:#}");
                    warn!(partition, error = %error, "skipping failed partition");
                    self.summary.skipped_count += 1;
                    emit_overall_progress(
                        &mut self.emit,
                        completed_before,
                        bytes,
                        self.overall_total,
                    )?;
                    (self.emit)(FlashEvent::PartitionSkipped {
                        partition: partition.to_string(),
                        operation,
                        reason,
                    })?;
                    Ok(PartitionFlashOutcome::Skipped)
                }
                _ => {
                    let msg = format!("{error:#}");
                    (self.emit)(FlashEvent::PartitionFailed {
                        partition: partition.to_string(),
                        operation,
                        error: msg.clone(),
                    })?;
                    Err(msg)
                }
            },
        }
    }

    /// Erase a partition and emit shared progress events.
    pub async fn erase_partition(
        &mut self,
        partition: &str,
        bytes: u64,
        completed_before: u64,
    ) -> Result<(), String> {
        self.control.ensure_not_cancelled()?;
        (self.emit)(FlashEvent::Erasing {
            partition: partition.to_string(),
        })?;
        emit_overall_progress(&mut self.emit, completed_before, 0, self.overall_total)?;

        match erase_one_partition(self.dev, partition).await {
            Ok(()) => {
                self.summary.wipe_count += 1;
                emit_overall_progress(&mut self.emit, completed_before, bytes, self.overall_total)?;
                (self.emit)(FlashEvent::EraseComplete {
                    partition: partition.to_string(),
                })?;
                Ok(())
            }
            Err(e) => {
                let msg = format!("{e:#}");
                (self.emit)(FlashEvent::PartitionFailed {
                    partition: partition.to_string(),
                    operation: FlashOperation::Erase,
                    error: msg.clone(),
                })?;
                Err(msg)
            }
        }
    }

    /// Execute a filtered set of scatter plan actions.
    pub async fn execute_plan_actions(
        &mut self,
        actions: &[&mtk_scatter_parser::FlashAction],
        image_overrides: &HashMap<String, String>,
        format_tools: Option<&FormatTools>,
        device_vars: &HashMap<String, String>,
    ) -> Result<(), String> {
        let mut completed_before = 0_u64;

        for action in actions {
            self.control.ensure_not_cancelled()?;
            let action_bytes = u64::try_from(action.size).unwrap_or(0);
            match action.execution_kind {
                FlashActionExecutionKind::Flash => {
                    let allow_skip = action_is_skip_eligible(action);
                    let image_path =
                        match crate::domain::resolve_image_path_for_action(action, image_overrides)
                        {
                            Ok(p) => p,
                            Err(e) if allow_skip => {
                                warn!(
                                    partition = action.partition,
                                    safety_class = action.safety_class.as_str(),
                                    "skipping partition with missing image path"
                                );
                                (self.emit)(FlashEvent::PartitionSkipped {
                                    partition: action.partition.clone(),
                                    operation: FlashOperation::Flash,
                                    reason: e,
                                })?;
                                self.summary.skipped_count += 1;
                                completed_before = completed_before.saturating_add(action_bytes);
                                continue;
                            }
                            Err(e) => return Err(e),
                        };
                    let partition = resolve_scatter_flash_target(&action.partition, device_vars);
                    let outcome = self
                        .flash_partition(
                            &partition,
                            &image_path,
                            action_bytes,
                            completed_before,
                            allow_skip,
                            FlashOperation::Flash,
                        )
                        .await?;
                    completed_before = completed_before.saturating_add(action_bytes);
                    if outcome == PartitionFlashOutcome::Skipped {
                        continue;
                    }
                }
                FlashActionExecutionKind::FormatUserdata => {
                    let Some(tools) = format_tools else {
                        return Err(
                            "missing format tools for clean-flash userdata wipe".to_string()
                        );
                    };
                    let info = detect_userdata(self.dev)
                        .await
                        .map_err(|error| format!("detect userdata: {error:#}"))?;
                    let generated =
                        generate_userdata_image(tools, &info, &FormatUserdataOptions::default())
                            .map_err(|error| format!("generate userdata image: {error:#}"))?;
                    self.control.ensure_not_cancelled()?;
                    (self.emit)(FlashEvent::PreparingImage {
                        partition: action.partition.clone(),
                        operation: FlashOperation::FormatUserdata,
                    })?;
                    emit_overall_progress(&mut self.emit, completed_before, 0, self.overall_total)?;
                    self.flash_one_partition_evented(
                        &action.partition,
                        generated.path(),
                        action_bytes.max(1),
                        completed_before,
                        FlashOperation::FormatUserdata,
                    )
                    .await
                    .map_err(|error| {
                        let msg = format!("{error:#}");
                        let _ = (self.emit)(FlashEvent::PartitionFailed {
                            partition: action.partition.clone(),
                            operation: FlashOperation::FormatUserdata,
                            error: msg.clone(),
                        });
                        msg
                    })?;
                    self.summary.wipe_count += 1;
                    emit_overall_progress(
                        &mut self.emit,
                        completed_before,
                        action_bytes.max(1),
                        self.overall_total,
                    )?;
                    (self.emit)(FlashEvent::PartitionComplete {
                        partition: action.partition.clone(),
                        operation: FlashOperation::FormatUserdata,
                    })?;
                    completed_before = completed_before.saturating_add(action_bytes);
                }
                FlashActionExecutionKind::EraseOptional => {
                    match erase_one_partition(self.dev, &action.partition).await {
                        Ok(()) => {
                            self.summary.wipe_count += 1;
                            emit_overall_progress(
                                &mut self.emit,
                                completed_before,
                                action_bytes,
                                self.overall_total,
                            )?;
                            (self.emit)(FlashEvent::EraseComplete {
                                partition: action.partition.clone(),
                            })?;
                            completed_before = completed_before.saturating_add(action_bytes);
                        }
                        Err(error) if wipe_failure_is_skip_eligible(action, &error) => {
                            let reason = format!("{error:#}");
                            warn!(partition = action.partition, error = %error, "skipping failed wipe");
                            self.summary.skipped_count += 1;
                            emit_overall_progress(
                                &mut self.emit,
                                completed_before,
                                action_bytes,
                                self.overall_total,
                            )?;
                            (self.emit)(FlashEvent::PartitionSkipped {
                                partition: action.partition.clone(),
                                operation: FlashOperation::Erase,
                                reason,
                            })?;
                            completed_before = completed_before.saturating_add(action_bytes);
                        }
                        Err(error) => {
                            let msg = format!("{error:#}");
                            (self.emit)(FlashEvent::PartitionFailed {
                                partition: action.partition.clone(),
                                operation: FlashOperation::Erase,
                                error: msg.clone(),
                            })?;
                            return Err(msg);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn flash_one_partition_evented(
        &mut self,
        partition: &str,
        image: &Path,
        total_bytes: u64,
        completed_before: u64,
        operation: FlashOperation,
    ) -> Result<(), anyhow::Error> {
        let p = partition.to_string();
        let p2 = p.clone();
        let emit_partition = p.clone();
        let overall_total = self.overall_total;
        let mut bytes_flashed: u64 = 0;
        let start = std::time::Instant::now();
        let emit = &mut self.emit;

        flash_one_partition(self.dev, &p2, image, self.max_download_size, move |event| {
            if let FlashProgress::DownloadBytes { bytes, .. } = event {
                bytes_flashed += bytes;
                let speed_bps = {
                    let secs = start.elapsed().as_secs_f64();
                    if secs > 0.0 {
                        (bytes_flashed as f64 / secs) as u64
                    } else {
                        0
                    }
                };
                let _ = emit(FlashEvent::Flashing {
                    partition: emit_partition.clone(),
                    operation,
                    bytes: bytes_flashed,
                    total: total_bytes.max(1),
                    speed_bps,
                });
                let _ = emit_overall_progress(emit, completed_before, bytes_flashed, overall_total);
            }
        })
        .await?;
        Ok(())
    }
}

fn resolve_scatter_flash_target(partition: &str, device_vars: &HashMap<String, String>) -> String {
    resolve_flash_partition_target(partition, device_vars)
}

fn emit_overall_progress<E>(
    emit: &mut E,
    completed_before: u64,
    current_bytes: u64,
    total_bytes: u64,
) -> Result<(), String>
where
    E: FnMut(FlashEvent) -> Result<(), String>,
{
    let (bytes, total) = update_overall_progress(completed_before, current_bytes, total_bytes);
    emit(FlashEvent::Overall { bytes, total })
}

/// Build progress events for a dry-run plan.
pub async fn simulate_dry_run_actions(
    actions: &[&mtk_scatter_parser::FlashAction],
    control: &FlashRunControl,
    emit: &mut impl FnMut(FlashEvent) -> Result<(), String>,
    summary: &mut FlashSummaryDto,
    overall_total: u64,
) -> Result<(), String> {
    let mut completed_before = 0_u64;

    for action in actions {
        control.ensure_not_cancelled()?;
        let partition = action.partition.clone();
        let total = u64::try_from(action.size).unwrap_or(0).max(1);
        let mut completed: u64 = 0;
        let operation = match action.execution_kind {
            FlashActionExecutionKind::Flash => FlashOperation::Flash,
            FlashActionExecutionKind::FormatUserdata => FlashOperation::FormatUserdata,
            FlashActionExecutionKind::EraseOptional => FlashOperation::Erase,
        };

        match operation {
            FlashOperation::Flash | FlashOperation::FormatUserdata => {
                emit(FlashEvent::PreparingImage {
                    partition: partition.clone(),
                    operation,
                })?;

                for step in crate::progress::dry_run_steps(total, 1024) {
                    control.ensure_not_cancelled()?;
                    completed = completed.saturating_add(step.bytes);
                    emit_overall_progress(
                        emit,
                        completed_before,
                        completed.min(total),
                        overall_total,
                    )?;
                    emit(FlashEvent::Simulating {
                        partition: partition.clone(),
                        operation,
                        bytes: completed.min(total),
                        total,
                        speed_bps: 1024 * 1024 * 1024,
                    })?;
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }

                match operation {
                    FlashOperation::Flash => summary.flash_count += 1,
                    FlashOperation::FormatUserdata => summary.wipe_count += 1,
                    FlashOperation::Erase => {}
                }
                completed_before = completed_before.saturating_add(total);
                emit(FlashEvent::PartitionComplete {
                    partition,
                    operation,
                })?;
            }
            FlashOperation::Erase => {
                emit(FlashEvent::Erasing {
                    partition: partition.clone(),
                })?;

                for step in crate::progress::dry_run_steps(total, 1024) {
                    control.ensure_not_cancelled()?;
                    completed = completed.saturating_add(step.bytes);
                    emit_overall_progress(
                        emit,
                        completed_before,
                        completed.min(total),
                        overall_total,
                    )?;
                    emit(FlashEvent::Simulating {
                        partition: partition.clone(),
                        operation,
                        bytes: completed.min(total),
                        total,
                        speed_bps: 1024 * 1024 * 1024,
                    })?;
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }

                summary.wipe_count += 1;
                completed_before = completed_before.saturating_add(total);
                emit(FlashEvent::EraseComplete { partition })?;
            }
        }
    }

    Ok(())
}

/// Execute a scatter plan without a device connection.
pub async fn run_scatter_dry_run(
    plan: &mtk_scatter_parser::FlashPlan,
    partitions: &[String],
    control: &FlashRunControl,
    emit: &mut impl FnMut(FlashEvent) -> Result<(), String>,
) -> Result<FlashSummaryDto, String> {
    let actions = filter_actions(plan, partitions);
    let total_bytes = total_bytes_for_actions(&actions);

    emit(FlashEvent::PlanBuilt {
        actions: actions.len(),
        total_bytes,
    })?;
    emit(FlashEvent::Overall {
        bytes: 0,
        total: total_bytes,
    })?;

    let mut summary = FlashSummaryDto {
        flash_count: 0,
        wipe_count: 0,
        skipped_count: 0,
        total_bytes,
    };

    simulate_dry_run_actions(&actions, control, emit, &mut summary, total_bytes).await?;
    emit(FlashEvent::Complete {
        summary: summary.clone(),
    })?;
    Ok(summary)
}

/// Execute a scatter plan on a connected device.
pub async fn run_scatter_flash(
    dev: &mut FastbootDevice,
    plan: &mtk_scatter_parser::FlashPlan,
    options: ScatterFlashOptions<'_>,
    emit: &mut impl FnMut(FlashEvent) -> Result<(), String>,
) -> Result<FlashSummaryDto, String> {
    let actions = filter_actions(plan, options.partitions);
    let total_bytes = total_bytes_for_actions(&actions);

    if options.announce_plan {
        emit(FlashEvent::PlanBuilt {
            actions: actions.len(),
            total_bytes,
        })?;
    }
    emit(FlashEvent::Overall {
        bytes: 0,
        total: total_bytes,
    })?;

    let vars = read_all_variables(dev)
        .await
        .map_err(|e| format!("read vars: {e}"))?;
    let max_download_size = resolve_max_download_size_from_vars(&vars)
        .map_err(|e| format!("max-download-size: {e}"))?;

    let mut summary = FlashSummaryDto {
        flash_count: 0,
        wipe_count: 0,
        skipped_count: 0,
        total_bytes,
    };

    let mut flash = FlashProgressContext {
        dev,
        emit: &mut *emit,
        summary: &mut summary,
        control: options.control,
        max_download_size,
        overall_total: total_bytes,
    };
    flash
        .execute_plan_actions(
            &actions,
            options.image_overrides,
            options.format_tools,
            &vars,
        )
        .await?;

    if options.reboot {
        emit(FlashEvent::Rebooting {
            target: "system".to_string(),
        })?;
        reboot_device(dev)
            .await
            .map_err(|e| format!("reboot: {e}"))?;
    }

    emit(FlashEvent::Complete {
        summary: summary.clone(),
    })?;
    Ok(summary)
}

/// Execute a set of manual flash actions on a connected device.
pub async fn execute_manual_actions(
    actions: &[ManualFlashAction],
    dev: &mut FastbootDevice,
    execution: ManualActionExecution<'_>,
    emit: &mut impl FnMut(FlashEvent) -> Result<(), String>,
) -> Result<(), String> {
    let mut completed_before = 0_u64;

    for action in actions {
        execution.control.ensure_not_cancelled()?;
        let mut flash = FlashProgressContext {
            dev,
            emit: &mut *emit,
            summary: &mut *execution.summary,
            control: execution.control,
            max_download_size: execution.max_download_size,
            overall_total: execution.overall_total,
        };
        let partition = (execution.partition_resolver)(&action.partition);
        flash
            .flash_partition(
                &partition,
                &action.image,
                action.size,
                completed_before,
                false,
                FlashOperation::Flash,
            )
            .await?;
        completed_before = completed_before.saturating_add(action.size);
    }

    Ok(())
}

/// Generate and flash userdata using the same shared event stream.
pub async fn format_userdata_flow(
    dev: &mut FastbootDevice,
    tools: &FormatTools,
    options: &FormatUserdataOptions,
    control: &FlashRunControl,
    emit: &mut impl FnMut(FlashEvent) -> Result<(), String>,
) -> Result<FlashSummaryDto, String> {
    let info = detect_userdata(dev)
        .await
        .map_err(|e| format!("detect userdata: {e}"))?;
    let result = format_userdata_with_info_flow(dev, tools, info, options, control, emit).await?;
    Ok(result)
}

async fn format_userdata_with_info_flow(
    dev: &mut FastbootDevice,
    tools: &FormatTools,
    info: UserdataInfo,
    options: &FormatUserdataOptions,
    control: &FlashRunControl,
    emit: &mut impl FnMut(FlashEvent) -> Result<(), String>,
) -> Result<FlashSummaryDto, String> {
    let generated = match generate_userdata_image(tools, &info, options) {
        Ok(image) => image,
        Err(_error) if options.erase_fallback => {
            erase_one_partition(dev, "userdata")
                .await
                .context("erase userdata fallback")
                .map_err(|e| format!("{e:#}"))?;
            let summary = FlashSummaryDto {
                flash_count: 0,
                wipe_count: 1,
                skipped_count: 0,
                total_bytes: 1,
            };
            emit(FlashEvent::PlanBuilt {
                actions: 1,
                total_bytes: 1,
            })?;
            emit(FlashEvent::Overall { bytes: 0, total: 1 })?;
            emit(FlashEvent::Erasing {
                partition: "userdata".to_string(),
            })?;
            emit(FlashEvent::EraseComplete {
                partition: "userdata".to_string(),
            })?;
            emit(FlashEvent::Complete {
                summary: summary.clone(),
            })?;
            return Ok(summary);
        }
        Err(error) => return Err(error.to_string()),
    };

    let max_download = info
        .max_download_size
        .context("missing userdata max-download-size")
        .and_then(|value| {
            u32::try_from(value).context("userdata max-download-size exceeds supported range")
        })
        .map_err(|e| format!("{e:#}"))?;

    let total_bytes = generated
        .image_len()
        .map_err(|e| format!("generated image: {e}"))?;
    emit(FlashEvent::PlanBuilt {
        actions: 1,
        total_bytes,
    })?;
    emit(FlashEvent::Overall {
        bytes: 0,
        total: total_bytes,
    })?;

    let mut summary = FlashSummaryDto {
        flash_count: 0,
        wipe_count: 0,
        skipped_count: 0,
        total_bytes,
    };
    let mut flash = FlashProgressContext {
        dev,
        emit: &mut *emit,
        summary: &mut summary,
        control,
        max_download_size: max_download,
        overall_total: total_bytes,
    };
    flash
        .flash_partition(
            "userdata",
            generated.path(),
            total_bytes,
            0,
            false,
            FlashOperation::FormatUserdata,
        )
        .await?;

    emit(FlashEvent::Complete {
        summary: summary.clone(),
    })?;
    Ok(summary)
}

/// Wipe userdata plus optional metadata/cache partitions using shared events.
pub async fn wipe_data_flow(
    dev: &mut FastbootDevice,
    tools: &FormatTools,
    options: &WipeDataOptions,
    control: &FlashRunControl,
    emit: &mut impl FnMut(FlashEvent) -> Result<(), String>,
) -> Result<FlashSummaryDto, String> {
    let info = detect_userdata(dev)
        .await
        .map_err(|e| format!("detect userdata: {e}"))?;

    let format_options = FormatUserdataOptions {
        erase_fallback: options.erase_fallback,
        casefold: options.casefold,
    };
    let generated = generate_userdata_image(tools, &info, &format_options);
    let erase_steps = usize::from(options.erase_metadata) + usize::from(options.erase_cache);
    let base_bytes = match &generated {
        Ok(image) => image
            .image_len()
            .map_err(|e| format!("generated image: {e}"))?,
        Err(_) if options.erase_fallback => 1,
        Err(_) => 0,
    };
    let total_bytes = base_bytes + u64::try_from(erase_steps).unwrap_or(0);

    emit(FlashEvent::PlanBuilt {
        actions: 1 + erase_steps,
        total_bytes,
    })?;
    emit(FlashEvent::Overall {
        bytes: 0,
        total: total_bytes,
    })?;

    let mut summary = FlashSummaryDto {
        flash_count: 0,
        wipe_count: 0,
        skipped_count: 0,
        total_bytes,
    };

    match generated {
        Ok(image) => {
            let max_download_size = info
                .max_download_size
                .context("missing userdata max-download-size")
                .and_then(|value| {
                    u32::try_from(value)
                        .context("userdata max-download-size exceeds supported range")
                })
                .map_err(|e| format!("{e:#}"))?;
            let mut flash = FlashProgressContext {
                dev,
                emit: &mut *emit,
                summary: &mut summary,
                control,
                max_download_size,
                overall_total: total_bytes.max(1),
            };
            flash
                .flash_partition(
                    "userdata",
                    image.path(),
                    base_bytes.max(1),
                    0,
                    false,
                    FlashOperation::FormatUserdata,
                )
                .await?;
        }
        Err(_error) if options.erase_fallback => {
            let mut flash = FlashProgressContext {
                dev,
                emit: &mut *emit,
                summary: &mut summary,
                control,
                max_download_size: 0,
                overall_total: total_bytes.max(1),
            };
            flash
                .erase_partition("userdata", base_bytes.max(1), 0)
                .await?;
        }
        Err(error) => return Err(format!("generate userdata image: {error:#}")),
    }

    let mut completed_before = base_bytes.max(1);
    if options.erase_metadata {
        erase_optional_partition_and_emit(
            dev,
            emit,
            &mut summary,
            control,
            "metadata",
            completed_before,
            total_bytes.max(1),
        )
        .await?;
        completed_before = completed_before.saturating_add(1);
    }
    if options.erase_cache {
        erase_optional_partition_and_emit(
            dev,
            emit,
            &mut summary,
            control,
            "cache",
            completed_before,
            total_bytes.max(1),
        )
        .await?;
    }

    emit(FlashEvent::Complete {
        summary: summary.clone(),
    })?;
    Ok(summary)
}

async fn erase_optional_partition_and_emit(
    dev: &mut FastbootDevice,
    emit: &mut impl FnMut(FlashEvent) -> Result<(), String>,
    summary: &mut FlashSummaryDto,
    control: &FlashRunControl,
    partition: &'static str,
    completed_before: u64,
    overall_total: u64,
) -> Result<(), String> {
    control.ensure_not_cancelled()?;
    emit(FlashEvent::Erasing {
        partition: partition.to_string(),
    })?;
    emit_overall_progress(emit, completed_before, 0, overall_total)?;

    match erase_optional_partition(dev, partition)
        .await
        .map_err(|e| format!("erase {partition}: {e}"))?
    {
        OptionalEraseOutcome::Erased => {
            summary.wipe_count += 1;
            emit_overall_progress(emit, completed_before, 1, overall_total)?;
            emit(FlashEvent::EraseComplete {
                partition: partition.to_string(),
            })?;
        }
        OptionalEraseOutcome::Skipped { reason } => {
            summary.skipped_count += 1;
            emit_overall_progress(emit, completed_before, 1, overall_total)?;
            emit(FlashEvent::PartitionSkipped {
                partition: partition.to_string(),
                operation: FlashOperation::Erase,
                reason,
            })?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        action_is_skip_eligible, partition_flash_failure_disposition, resolve_scatter_flash_target,
        simulate_dry_run_actions, wipe_failure_is_skip_eligible, PartitionFlashFailureDisposition,
    };
    use fastboot_rs::{transport::nusb::NusbFastBootError, FastbootError, FastbootExecutionError};
    use serde_json::json;
    use std::collections::HashMap;

    use crate::{
        FlashAction, FlashActionExecutionKind, FlashEvent, FlashOperation, FlashRunControl,
        FlashSummaryDto,
    };

    fn test_action(partition: &str, safety_class: &str) -> FlashAction {
        FlashAction {
            action: "flash".to_string(),
            execution_kind: if partition == "userdata" {
                FlashActionExecutionKind::FormatUserdata
            } else if safety_class == "wipe_only" {
                FlashActionExecutionKind::EraseOptional
            } else {
                FlashActionExecutionKind::Flash
            },
            partition: partition.to_string(),
            base_name: partition.to_string(),
            slot: None,
            layout: "TEST".to_string(),
            region: "TEST".to_string(),
            start: 0,
            start_hex: "0x0".to_string(),
            size: 1,
            size_hex: "0x1".to_string(),
            size_human: "1 B".to_string(),
            image: Some(json!({})),
            image_type: None,
            safety_class: safety_class.to_string(),
            reason: "test".to_string(),
            warnings: Vec::new(),
        }
    }

    fn fastboot_failed_error() -> anyhow::Error {
        anyhow::Error::new(FastbootExecutionError::Fastboot(FastbootError::Nusb(
            NusbFastBootError::FastbootFailed("rejected".to_string()),
        )))
    }

    fn vars_with_partitions(parts: &[&str]) -> HashMap<String, String> {
        parts
            .iter()
            .map(|part| (format!("partition-size:{part}"), "0x1000".to_string()))
            .collect()
    }

    fn should_skip_failure(action: &FlashAction, error: &anyhow::Error) -> bool {
        action_is_skip_eligible(action)
            && matches!(
                partition_flash_failure_disposition(error),
                PartitionFlashFailureDisposition::Skip
            )
    }

    #[test]
    fn skip_eligibility_blocks_boot_critical_partitions() {
        assert!(!action_is_skip_eligible(&test_action(
            "boot",
            "boot_critical"
        )));
        assert!(!action_is_skip_eligible(&test_action(
            "vbmeta",
            "android_system"
        )));
    }

    #[test]
    fn skip_eligibility_allows_noncritical_partitions() {
        assert!(action_is_skip_eligible(&test_action("modem", "firmware")));
        assert!(action_is_skip_eligible(&test_action("logo", "regional")));
    }

    #[test]
    fn partition_flash_failure_disposition_skips_fastboot_failed_responses() {
        assert_eq!(
            partition_flash_failure_disposition(&fastboot_failed_error()),
            PartitionFlashFailureDisposition::Skip
        );
    }

    #[test]
    fn partition_flash_failure_disposition_keeps_other_errors_fatal() {
        assert_eq!(
            partition_flash_failure_disposition(&anyhow::Error::msg("boom")),
            PartitionFlashFailureDisposition::Fatal
        );
    }

    #[test]
    fn recoverable_failures_are_skippable_for_noncritical_partitions() {
        let action = test_action("modem", "firmware");

        assert!(should_skip_failure(&action, &fastboot_failed_error()));
    }

    #[test]
    fn recoverable_failures_remain_fatal_for_boot_critical_partitions() {
        let action = test_action("boot", "boot_critical");

        assert!(!should_skip_failure(&action, &fastboot_failed_error()));
    }

    #[test]
    fn wipe_failures_are_skippable_for_noncritical_partitions() {
        let action = test_action("cache", "wipe_only");

        assert!(wipe_failure_is_skip_eligible(
            &action,
            &fastboot_failed_error()
        ));
    }

    #[test]
    fn wipe_failures_remain_fatal_for_boot_critical_partitions() {
        let action = test_action("boot", "boot_critical");

        assert!(!wipe_failure_is_skip_eligible(
            &action,
            &fastboot_failed_error()
        ));
    }

    #[test]
    fn wipe_failures_remain_fatal_for_clean_flash_userdata() {
        let action = test_action("userdata", "wipe_only");

        assert!(!wipe_failure_is_skip_eligible(
            &action,
            &fastboot_failed_error()
        ));
    }

    #[test]
    fn resolve_scatter_flash_target_keeps_exact_partition_when_available() {
        let vars = vars_with_partitions(&["vbmeta_a", "vbmeta"]);

        assert_eq!(resolve_scatter_flash_target("vbmeta_a", &vars), "vbmeta_a");
    }

    #[test]
    fn resolve_scatter_flash_target_falls_back_to_unsuffixed_partition() {
        let vars = vars_with_partitions(&["vbmeta"]);

        assert_eq!(resolve_scatter_flash_target("vbmeta_a", &vars), "vbmeta");
    }

    #[tokio::test]
    async fn simulate_dry_run_actions_counts_format_userdata_as_wipe() {
        let actions = [FlashAction {
            action: "wipe".to_string(),
            execution_kind: FlashActionExecutionKind::FormatUserdata,
            partition: "userdata".to_string(),
            base_name: "userdata".to_string(),
            slot: None,
            layout: "TEST".to_string(),
            region: "TEST".to_string(),
            start: 0,
            start_hex: "0x0".to_string(),
            size: 4,
            size_hex: "0x4".to_string(),
            size_human: "4 B".to_string(),
            image: None,
            image_type: None,
            safety_class: "wipe_only".to_string(),
            reason: "test".to_string(),
            warnings: Vec::new(),
        }];
        let action_refs = actions.iter().collect::<Vec<_>>();
        let control = FlashRunControl::default();
        let mut summary = FlashSummaryDto {
            flash_count: 0,
            wipe_count: 0,
            skipped_count: 0,
            total_bytes: 4,
        };
        let mut events = Vec::new();
        let mut emit = |event: FlashEvent| -> Result<(), String> {
            events.push(event);
            Ok(())
        };

        simulate_dry_run_actions(&action_refs, &control, &mut emit, &mut summary, 4)
            .await
            .unwrap();

        assert_eq!(summary.flash_count, 0);
        assert_eq!(summary.wipe_count, 1);
        assert!(events.iter().any(|event| {
            matches!(
                event,
                FlashEvent::PreparingImage {
                    partition,
                    operation: FlashOperation::FormatUserdata,
                } if partition == "userdata"
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                FlashEvent::PartitionComplete {
                    partition,
                    operation: FlashOperation::FormatUserdata,
                } if partition == "userdata"
            )
        }));
    }
}
