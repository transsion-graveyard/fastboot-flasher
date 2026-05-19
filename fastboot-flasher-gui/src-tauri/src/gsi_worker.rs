use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use tauri::Emitter;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::{sleep, Duration};

use fastboot_flasher_core as fastboot_flasher;
use fastboot_flasher_core::{
    format::{detect_userdata, FormatTools, WipeDataOptions},
    gsi::{
        build_gsi_execution_plan, detect_fastboot_mode, execute_gsi_flash_with_vars,
        inspect_gsi_image, maybe_needs_product_gsi, GsiEvent, GsiFlashOptions, GsiFlashSummary,
        GsiStep,
    },
    manual::resolved_disable_vbmeta_image_path,
};

use crate::{
    build_format_tools, format_tools_platform, resolve_format_tools, AppState, FlashEvent,
    FlashRunControl, FlashSummaryDto, CANCELLED_MESSAGE,
};

pub(crate) const GSI_WORKER_ARG: &str = "--gsi-worker";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct GsiWorkerRequest {
    pub(crate) image: String,
    pub(crate) tools_root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "data")]
pub(crate) enum GsiWorkerMessage {
    Event(FlashEvent),
    Result(GsiWorkerResult),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", content = "data")]
pub(crate) enum GsiWorkerResult {
    Complete { summary: FlashSummaryDto },
    Failed { message: String },
}

pub(crate) struct GsiWorkerProgressMapper {
    completed_before: u64,
    total_bytes: u64,
}

impl GsiWorkerProgressMapper {
    pub(crate) fn new(total_bytes: u64) -> Self {
        Self {
            completed_before: 0,
            total_bytes,
        }
    }

    pub(crate) fn map_event(&mut self, event: GsiEvent) -> Vec<FlashEvent> {
        match event {
            GsiEvent::Step(step) => vec![FlashEvent::GsiStatus {
                status: gsi_step_status(step).to_string(),
            }],
            GsiEvent::ModeDetected(mode) => vec![FlashEvent::GsiStatus {
                status: gsi_mode_status(mode.as_str(), false).to_string(),
            }],
            GsiEvent::ModeReady(mode) => vec![FlashEvent::GsiStatus {
                status: gsi_mode_status(mode.as_str(), true).to_string(),
            }],
            GsiEvent::UserdataEraseFallback { .. } => vec![FlashEvent::GsiStatus {
                status: "userdata_erase_fallback".to_string(),
            }],
            GsiEvent::ResolvedPartition { .. } => vec![],
            GsiEvent::Flashing {
                partition,
                size_bytes,
                ..
            } => vec![
                FlashEvent::PreparingImage {
                    partition: partition.clone(),
                },
                FlashEvent::Overall {
                    bytes: self.completed_before.min(self.total_bytes),
                    total: self.total_bytes,
                },
                FlashEvent::Flashing {
                    partition,
                    bytes: 0,
                    total: size_bytes.max(1),
                    speed_bps: 0,
                },
            ],
            GsiEvent::FlashProgress {
                partition,
                bytes,
                total_bytes,
                speed_bps,
            } => vec![
                FlashEvent::Flashing {
                    partition,
                    bytes,
                    total: total_bytes.max(1),
                    speed_bps,
                },
                FlashEvent::Overall {
                    bytes: self
                        .completed_before
                        .saturating_add(bytes)
                        .min(self.total_bytes),
                    total: self.total_bytes,
                },
            ],
            GsiEvent::FlashFinished {
                partition,
                size_bytes,
            } => {
                self.completed_before = self.completed_before.saturating_add(size_bytes);
                vec![
                    FlashEvent::Overall {
                        bytes: self.completed_before.min(self.total_bytes),
                        total: self.total_bytes,
                    },
                    FlashEvent::PartitionComplete { partition },
                ]
            }
            GsiEvent::Erasing { partition } => vec![
                FlashEvent::Erasing {
                    partition: partition.to_string(),
                },
                FlashEvent::Overall {
                    bytes: self.completed_before.min(self.total_bytes),
                    total: self.total_bytes,
                },
            ],
            GsiEvent::EraseFinished { partition } => {
                self.completed_before = self.completed_before.saturating_add(1);
                vec![
                    FlashEvent::Overall {
                        bytes: self.completed_before.min(self.total_bytes),
                        total: self.total_bytes,
                    },
                    FlashEvent::EraseComplete {
                        partition: partition.to_string(),
                    },
                ]
            }
            GsiEvent::PartitionSkipped { partition, reason } => {
                self.completed_before = self.completed_before.saturating_add(1);
                vec![
                    FlashEvent::Overall {
                        bytes: self.completed_before.min(self.total_bytes),
                        total: self.total_bytes,
                    },
                    FlashEvent::PartitionSkipped {
                        partition: partition.to_string(),
                        reason,
                    },
                ]
            }
        }
    }
}

pub(crate) fn is_gsi_worker_invocation(args: &[String]) -> bool {
    args.get(1).map(String::as_str) == Some(GSI_WORKER_ARG)
}

pub(crate) fn run_gsi_worker_stdio() -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("read worker stdin: {e}"))?;
    let request: GsiWorkerRequest =
        serde_json::from_str(&input).map_err(|e| format!("parse worker request: {e}"))?;

    let runtime =
        tokio::runtime::Runtime::new().map_err(|e| format!("start tokio runtime: {e}"))?;
    runtime.block_on(async move {
        let mut sink = std::io::stdout().lock();
        match execute_gsi_worker_request(request, |message| {
            write_worker_message(&mut sink, message)
        })
        .await
        {
            Ok(summary) => write_worker_message(
                &mut sink,
                GsiWorkerMessage::Result(GsiWorkerResult::Complete { summary }),
            ),
            Err(message) => {
                let _ = write_worker_message(
                    &mut sink,
                    GsiWorkerMessage::Result(GsiWorkerResult::Failed {
                        message: message.clone(),
                    }),
                );
                Err(message)
            }
        }
    })
}

pub(crate) async fn run_gsi_worker_and_emit(
    _state: &AppState,
    app: &tauri::AppHandle,
    control: &FlashRunControl,
    image: String,
) -> Result<FlashSummaryDto, String> {
    let tools = resolve_format_tools(app)?;
    let request = GsiWorkerRequest {
        image,
        tools_root: tools.root.display().to_string(),
    };
    let request_json =
        serde_json::to_vec(&request).map_err(|e| format!("serialize GSI worker request: {e}"))?;
    let current_exe = std::env::current_exe().map_err(|e| format!("resolve current exe: {e}"))?;
    let mut child = Command::new(current_exe)
        .arg(GSI_WORKER_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("spawn GSI worker: {e}"))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "spawn GSI worker: missing stdin".to_string())?;
    stdin
        .write_all(&request_json)
        .await
        .map_err(|e| format!("write GSI worker request: {e}"))?;
    stdin
        .shutdown()
        .await
        .map_err(|e| format!("close GSI worker stdin: {e}"))?;
    drop(stdin);

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "spawn GSI worker: missing stdout".to_string())?;
    let mut lines = BufReader::new(stdout).lines();
    let mut summary = None;
    let mut worker_error = None;
    let mut kill_sent = false;

    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line.map_err(|e| format!("read GSI worker stdout: {e}"))? {
                    Some(line) => {
                        let message: GsiWorkerMessage = serde_json::from_str(&line)
                            .map_err(|e| format!("parse GSI worker message: {e}; line={line}"))?;
                        match message {
                            GsiWorkerMessage::Event(event) => {
                                app.emit("flash-progress", event)
                                    .map_err(|e| format!("emit: {e}"))?;
                            }
                            GsiWorkerMessage::Result(GsiWorkerResult::Complete { summary: worker_summary }) => {
                                summary = Some(worker_summary);
                            }
                            GsiWorkerMessage::Result(GsiWorkerResult::Failed { message }) => {
                                worker_error = Some(message);
                            }
                        }
                    }
                    None => break,
                }
            }
            _ = sleep(Duration::from_millis(50)) => {
                if control.cancel_requested.load(Ordering::SeqCst) && !kill_sent {
                    kill_sent = true;
                    let _ = child.start_kill();
                }
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("wait for GSI worker: {e}"))?;

    if control.cancel_requested.load(Ordering::SeqCst) {
        return Err(CANCELLED_MESSAGE.to_string());
    }

    if let Some(message) = worker_error {
        return Err(message);
    }

    if !status.success() {
        return Err(format!("GSI worker exited with status {status}"));
    }

    let summary = summary.ok_or_else(|| "GSI worker completed without summary".to_string())?;
    app.emit(
        "flash-progress",
        FlashEvent::Complete {
            summary: summary.clone(),
        },
    )
    .map_err(|e| format!("emit: {e}"))?;
    Ok(summary)
}

pub(crate) fn gsi_worker_connect_retry_delay() -> Duration {
    #[cfg(target_os = "windows")]
    {
        Duration::from_millis(250)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Duration::ZERO
    }
}

async fn execute_gsi_worker_request(
    request: GsiWorkerRequest,
    mut emit: impl FnMut(GsiWorkerMessage) -> Result<(), String>,
) -> Result<FlashSummaryDto, String> {
    emit(GsiWorkerMessage::Event(FlashEvent::WaitingForDevice))?;

    let startup_delay = gsi_worker_connect_retry_delay();
    if !startup_delay.is_zero() {
        sleep(startup_delay).await;
    }

    let mut dev = fastboot_flasher::connect_fastboot()
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let vars = fastboot_flasher::read_all_variables(&mut dev)
        .await
        .map_err(|e| format!("read vars: {e}"))?;
    let start_mode = detect_fastboot_mode(&vars);
    emit(GsiWorkerMessage::Event(FlashEvent::GsiStatus {
        status: gsi_mode_status(start_mode.as_str(), false).to_string(),
    }))?;

    let image_path = PathBuf::from(&request.image);
    let image_metadata =
        std::fs::metadata(&image_path).map_err(|e| format!("read GSI image metadata: {e}"))?;
    if !image_metadata.is_file() {
        return Err(format!("{} is not a regular file", image_path.display()));
    }

    let tools = build_worker_format_tools(Path::new(&request.tools_root))?;
    let userdata_info = detect_userdata(&mut dev)
        .await
        .map_err(|e| format!("detect userdata: {e}"))?;
    let inspected =
        inspect_gsi_image(&image_path).map_err(|e| format!("inspect GSI image: {e}"))?;
    let empty_vbmeta =
        resolved_disable_vbmeta_image_path().map_err(|e| format!("vbmeta path: {e}"))?;
    let empty_vbmeta_size = std::fs::metadata(&empty_vbmeta)
        .map_err(|e| format!("read vbmeta metadata: {e}"))?
        .len();
    let needs_product_gsi = maybe_needs_product_gsi(&mut dev, &vars, inspected.expanded_size)
        .await
        .map_err(|e| format!("check product_gsi fallback: {e}"))?;
    let gsi_options = GsiFlashOptions {
        wipe_data: WipeDataOptions::default(),
        cancel_token: None,
    };
    let execution_plan = build_gsi_execution_plan(
        start_mode,
        image_metadata.len(),
        empty_vbmeta_size,
        &userdata_info,
        &gsi_options,
        needs_product_gsi,
    );
    emit(GsiWorkerMessage::Event(FlashEvent::PlanBuilt {
        actions: execution_plan.summary.flash_count
            + execution_plan.summary.wipe_count
            + execution_plan.summary.skipped_count,
        total_bytes: execution_plan.summary.total_bytes,
    }))?;
    emit(GsiWorkerMessage::Event(FlashEvent::Overall {
        bytes: 0,
        total: execution_plan.summary.total_bytes,
    }))?;

    let mut mapper = GsiWorkerProgressMapper::new(execution_plan.summary.total_bytes);
    let outcome =
        execute_gsi_flash_with_vars(dev, vars, &image_path, &tools, &gsi_options, |event| {
            for mapped in mapper.map_event(event) {
                let _ = emit(GsiWorkerMessage::Event(mapped));
            }
        })
        .await
        .map_err(|e| format!("{e:#}"))?;
    drop(outcome.device);
    Ok(flash_summary_from_gsi(outcome.summary))
}

fn write_worker_message(sink: &mut impl Write, message: GsiWorkerMessage) -> Result<(), String> {
    serde_json::to_writer(&mut *sink, &message)
        .map_err(|e| format!("encode GSI worker message: {e}"))?;
    sink.write_all(b"\n")
        .and_then(|_| sink.flush())
        .map_err(|e| format!("flush GSI worker message: {e}"))
}

fn build_worker_format_tools(root: &Path) -> Result<FormatTools, String> {
    let platform = format_tools_platform()?;
    Ok(build_format_tools(root.to_path_buf(), platform))
}

fn flash_summary_from_gsi(summary: GsiFlashSummary) -> FlashSummaryDto {
    FlashSummaryDto {
        flash_count: summary.flash_count,
        wipe_count: summary.wipe_count,
        skipped_count: summary.skipped_count,
        total_bytes: summary.total_bytes,
    }
}

fn gsi_mode_status(mode: &str, ready: bool) -> &'static str {
    match (mode, ready) {
        ("bootloader", false) => "bootloader_pending",
        ("bootloader", true) => "bootloader_ready",
        ("fastbootd", false) => "fastbootd_pending",
        ("fastbootd", true) => "fastbootd_ready",
        _ => "bootloader_pending",
    }
}

fn gsi_step_status(step: GsiStep) -> &'static str {
    match step {
        GsiStep::RebootingToBootloader => "rebooting_bootloader",
        GsiStep::RebootingToFastbootd => "rebooting_fastbootd",
        GsiStep::PreparingVbmetaFlash => "preparing_vbmeta",
        GsiStep::FlashingVbmeta => "flashing_vbmeta",
        GsiStep::CheckingSystemPartition => "checking_system_partition",
        GsiStep::CheckingProductGsiFallback => "checking_product_gsi",
        GsiStep::GeneratingProductGsiImage => "generating_product_gsi",
        GsiStep::FlashingProductGsi => "flashing_product_gsi",
        GsiStep::ProductGsiFallbackNotNeeded => "product_gsi_not_needed",
        GsiStep::FlashingSystemGsi => "flashing_system_gsi",
        GsiStep::WipingUserdata => "wiping_userdata",
        GsiStep::StartingBootloaderPhase => "bootloader_phase",
        GsiStep::StartingFastbootdPhase => "fastbootd_phase",
        GsiStep::GsiFlowComplete => "gsi_complete",
    }
}

#[cfg(test)]
mod tests {
    use crate::{FlashEvent, FlashSummaryDto};
    use fastboot_flasher::gsi::{GsiEvent, GsiStep};
    use tokio::time::Duration;

    use super::{
        gsi_worker_connect_retry_delay, GsiWorkerMessage, GsiWorkerProgressMapper,
        GsiWorkerRequest, GsiWorkerResult,
    };

    #[test]
    fn gsi_worker_request_round_trips_as_json() {
        let request = GsiWorkerRequest {
            image: "/tmp/system.img".to_string(),
            tools_root: "/tmp/tools".to_string(),
        };

        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: GsiWorkerRequest = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded.image, request.image);
        assert_eq!(decoded.tools_root, request.tools_root);
    }

    #[test]
    fn gsi_worker_message_round_trips_summary() {
        let message = GsiWorkerMessage::Result(GsiWorkerResult::Complete {
            summary: FlashSummaryDto {
                flash_count: 3,
                wipe_count: 2,
                skipped_count: 0,
                total_bytes: 123,
            },
        });

        let encoded = serde_json::to_string(&message).unwrap();
        let decoded: GsiWorkerMessage = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, message);
    }

    #[test]
    fn gsi_worker_progress_mapper_converts_flash_progress_and_completion() {
        let mut mapper = GsiWorkerProgressMapper::new(1000);

        let started = mapper.map_event(GsiEvent::Flashing {
            partition: "system_a".to_string(),
            image: "/tmp/system.img".into(),
            size_bytes: 1000,
        });
        assert_eq!(
            started,
            vec![
                FlashEvent::PreparingImage {
                    partition: "system_a".to_string()
                },
                FlashEvent::Overall {
                    bytes: 0,
                    total: 1000
                },
                FlashEvent::Flashing {
                    partition: "system_a".to_string(),
                    bytes: 0,
                    total: 1000,
                    speed_bps: 0,
                }
            ]
        );

        let progressed = mapper.map_event(GsiEvent::FlashProgress {
            partition: "system_a".to_string(),
            bytes: 400,
            total_bytes: 1000,
            speed_bps: 55,
        });
        assert_eq!(
            progressed,
            vec![
                FlashEvent::Flashing {
                    partition: "system_a".to_string(),
                    bytes: 400,
                    total: 1000,
                    speed_bps: 55,
                },
                FlashEvent::Overall {
                    bytes: 400,
                    total: 1000,
                }
            ]
        );

        let finished = mapper.map_event(GsiEvent::FlashFinished {
            partition: "system_a".to_string(),
            size_bytes: 1000,
        });
        assert_eq!(
            finished,
            vec![
                FlashEvent::Overall {
                    bytes: 1000,
                    total: 1000,
                },
                FlashEvent::PartitionComplete {
                    partition: "system_a".to_string(),
                }
            ]
        );
    }

    #[test]
    fn gsi_worker_progress_mapper_converts_status_steps() {
        let mut mapper = GsiWorkerProgressMapper::new(10);

        let events = mapper.map_event(GsiEvent::Step(GsiStep::FlashingSystemGsi));

        assert_eq!(
            events,
            vec![FlashEvent::GsiStatus {
                status: "flashing_system_gsi".to_string(),
            }]
        );
    }

    #[test]
    fn gsi_worker_connect_retry_delay_matches_platform_policy() {
        #[cfg(target_os = "windows")]
        assert_eq!(gsi_worker_connect_retry_delay(), Duration::from_millis(250));

        #[cfg(not(target_os = "windows"))]
        assert_eq!(gsi_worker_connect_retry_delay(), Duration::ZERO);
    }
}
