#![allow(missing_docs)]

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use fastboot_rs::{FastbootDevice, FlashProgress};
use mtk_scatter_parser::FlashAction;

use tracing::warn;

use crate::{
    device::{read_all_variables, reboot_device, resolve_max_download_size_from_vars},
    flash::{erase_one_partition, flash_one_partition, is_scatter_skippable_error},
    format::{
        detect_userdata, erase_optional_partition, generate_userdata_image, FormatTools,
        FormatUserdataOptions, OptionalEraseOutcome, UserdataInfo, WipeDataOptions,
    },
    manual::ManualFlashAction,
};

use crate::domain::{
    filter_actions, total_bytes_for_actions, update_overall_progress, FlashEvent, FlashRunControl,
    FlashSummaryDto,
};

/// Outcome of flashing a single partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionFlashOutcome {
    Completed,
    Skipped,
}

/// Whether a partition flash failure can be skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionFlashFailureDisposition {
    Skip,
    Fatal,
}

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
    action_is_skip_eligible(action) && is_scatter_skippable_error(error)
}

/// Progress context for flash operations that emit shared events.
pub struct FlashProgressContext<'a, E>
where
    E: FnMut(FlashEvent) -> Result<(), String>,
{
    pub dev: &'a mut FastbootDevice,
    pub emit: E,
    pub summary: &'a mut FlashSummaryDto,
    pub control: &'a FlashRunControl,
    pub max_download_size: u32,
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
    ) -> Result<PartitionFlashOutcome, String> {
        self.control.ensure_not_cancelled()?;
        (self.emit)(FlashEvent::PreparingImage {
            partition: partition.to_string(),
        })?;

        emit_overall_progress(&mut self.emit, completed_before, 0, self.overall_total)?;

        let result = self
            .flash_one_partition_evented(partition, image_path, bytes, completed_before)
            .await;

        match result {
            Ok(()) => {
                self.summary.flash_count += 1;
                emit_overall_progress(&mut self.emit, completed_before, bytes, self.overall_total)?;
                (self.emit)(FlashEvent::PartitionComplete {
                    partition: partition.to_string(),
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
                        reason,
                    })?;
                    Ok(PartitionFlashOutcome::Skipped)
                }
                _ => {
                    let msg = format!("{error:#}");
                    (self.emit)(FlashEvent::PartitionFailed {
                        partition: partition.to_string(),
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
    ) -> Result<(), String> {
        let mut completed_before = 0_u64;

        for action in actions {
            self.control.ensure_not_cancelled()?;
            let action_bytes = u64::try_from(action.size).unwrap_or(0);
            match action.action.as_str() {
                "flash" => {
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
                                    reason: e,
                                })?;
                                self.summary.skipped_count += 1;
                                completed_before = completed_before.saturating_add(action_bytes);
                                continue;
                            }
                            Err(e) => return Err(e),
                        };
                    let outcome = self
                        .flash_partition(
                            &action.partition,
                            &image_path,
                            action_bytes,
                            completed_before,
                            allow_skip,
                        )
                        .await?;
                    completed_before = completed_before.saturating_add(action_bytes);
                    if outcome == PartitionFlashOutcome::Skipped {
                        continue;
                    }
                }
                "wipe" => match erase_one_partition(self.dev, &action.partition).await {
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
                            reason,
                        })?;
                        completed_before = completed_before.saturating_add(action_bytes);
                    }
                    Err(error) => {
                        let msg = format!("{error:#}");
                        (self.emit)(FlashEvent::PartitionFailed {
                            partition: action.partition.clone(),
                            error: msg.clone(),
                        })?;
                        return Err(msg);
                    }
                },
                other => return Err(format!("unsupported plan action: {other}")),
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

        match action.action.as_str() {
            "flash" => {
                emit(FlashEvent::PreparingImage {
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
                        action: "flash".to_string(),
                        bytes: completed.min(total),
                        total,
                        speed_bps: 1024 * 1024 * 1024,
                    })?;
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }

                summary.flash_count += 1;
                completed_before = completed_before.saturating_add(total);
                emit(FlashEvent::PartitionComplete { partition })?;
            }
            "wipe" => {
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
                        action: "wipe".to_string(),
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
            other => return Err(format!("unsupported plan action: {other}")),
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
    partitions: &[String],
    image_overrides: &HashMap<String, String>,
    reboot: bool,
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
        control,
        max_download_size,
        overall_total: total_bytes,
    };
    flash
        .execute_plan_actions(&actions, image_overrides)
        .await?;

    if reboot {
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
    max_download_size: u32,
    control: &FlashRunControl,
    emit: &mut impl FnMut(FlashEvent) -> Result<(), String>,
    summary: &mut FlashSummaryDto,
    overall_total: u64,
) -> Result<(), String> {
    let mut completed_before = 0_u64;

    for action in actions {
        control.ensure_not_cancelled()?;
        let mut flash = FlashProgressContext {
            dev,
            emit: &mut *emit,
            summary,
            control,
            max_download_size,
            overall_total,
        };
        flash
            .flash_partition(
                &action.partition,
                &action.image,
                action.size,
                completed_before,
                false,
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
        .flash_partition("userdata", generated.path(), total_bytes, 0, false)
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
                .flash_partition("userdata", image.path(), base_bytes.max(1), 0, false)
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
                reason,
            })?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        action_is_skip_eligible, partition_flash_failure_disposition,
        wipe_failure_is_skip_eligible, PartitionFlashFailureDisposition,
    };
    use fastboot_rs::{transport::nusb::NusbFastBootError, FastbootError, FastbootExecutionError};
    use serde_json::json;

    use crate::FlashAction;

    fn test_action(partition: &str, safety_class: &str) -> FlashAction {
        FlashAction {
            action: "flash".to_string(),
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
}
