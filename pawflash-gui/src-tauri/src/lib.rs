mod gsi_worker;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{path::BaseDirectory, Emitter, Manager};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration, Instant};
use tracing::{debug, warn};

use pawflash as pawflash;
use pawflash::{
    cli::{FlashMode, SlotArg},
    format::{
        detect_userdata, erase_optional_partition, generate_userdata_image, FormatTools,
        FormatUserdataOptions, OptionalEraseOutcome, WipeDataOptions,
    },
    manual::{disable_vbmeta_actions, standalone_disable_vbmeta_path},
    progress::dry_run_steps,
    resolve_max_download_size_from_vars, FastbootDevice, FlashPlan,
};
use fastboot_rs::{open_fastboot_with_observer, BackendKind, FlashProgress, ProbeLogLevel};

const CANCELLED_MESSAGE: &str = "cancelled by user";
const DEVICE_CHECK_TIMEOUT_MS: u64 = 120_000;
const DEVICE_RETRY_DELAY_MS: u64 = 250;
const WINDOWS_FASTBOOTD_DRIVER_HINT: &str =
    "On Windows, fastbootd may need a different USB driver than bootloader mode.";
const POWER_OFF_UNSUPPORTED_MESSAGE: &str =
    "Power off is not supported by this device in the current fastboot mode.";

pub fn is_gsi_worker_invocation(args: &[String]) -> bool {
    gsi_worker::is_gsi_worker_invocation(args)
}

pub fn run_gsi_worker_stdio() -> Result<(), String> {
    gsi_worker::run_gsi_worker_stdio()
}

/// Initialize a default tracing subscriber for both the Tauri app and the
/// GSI worker process.
pub fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();
}

fn format_tools_platform() -> Result<&'static str, String> {
    #[cfg(target_os = "windows")]
    {
        Ok("windows")
    }
    #[cfg(target_os = "linux")]
    {
        Ok("linux")
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        Err("format tools are only supported on Linux and Windows hosts".to_string())
    }
}

fn build_format_tools(root: PathBuf, platform: &str) -> FormatTools {
    let dir = root.join(platform);
    let exe = if platform == "windows" { ".exe" } else { "" };

    FormatTools {
        mke2fs: dir.join(format!("mke2fs{exe}")),
        make_f2fs: dir.join(format!("make_f2fs{exe}")),
        make_f2fs_casefold: dir.join(format!("make_f2fs_casefold{exe}")),
        mke2fs_conf: dir.join("mke2fs.conf"),
        dir,
        root,
    }
}

struct AppState {
    device: DeviceCache,
    flash_plans: Mutex<StoredPlans>,
    flash_control: FlashRunControl,
    force_fastboot: Mutex<ForceFastbootState>,
    flash_in_progress: AtomicBool,
}

type DeviceCache = Mutex<Option<FastbootDevice>>;

struct StoredPlans {
    next_id: u64,
    plans: HashMap<u64, FlashPlan>,
}

#[derive(Clone, Default)]
struct FlashRunControl {
    cancel_requested: Arc<AtomicBool>,
}

#[derive(Default)]
struct ForceFastbootState {
    next_session_id: u64,
    active_session_id: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceSessionPolicy {
    ReuseCached,
    Fresh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FastbootProbeFailure {
    NoFastbootInterface,
    OpenFailed(String),
    ReadVariablesFailed(String),
}

#[derive(Clone, Serialize)]
pub struct DeviceInfo {
    serial: String,
    product: String,
    slot: String,
    secure: String,
    unlocked: String,
    version: String,
    all_vars: HashMap<String, String>,
}

#[derive(Clone, Serialize)]
pub struct PartitionDto {
    index: usize,
    action: String,
    partition: String,
    size_human: String,
    size_bytes: u64,
    safety_class: String,
    source: String,
    image_path: Option<String>,
    image_name: Option<String>,
    selected: bool,
}

#[derive(Clone, Serialize)]
pub struct FlashPlanDto {
    mode: String,
    storage: String,
    slot_policy: String,
    chipset: Option<String>,
    summary: FlashSummaryDto,
    partitions: Vec<PartitionDto>,
    warnings: Vec<String>,
    errors: Vec<String>,
}

#[derive(Clone, Serialize)]
pub struct ParseScatterResponseDto {
    plan_id: u64,
    plan: FlashPlanDto,
}

#[derive(Clone, Serialize)]
pub struct ForceFastbootStartDto {
    session_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FlashSummaryDto {
    flash_count: usize,
    wipe_count: usize,
    skipped_count: usize,
    total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", content = "data")]
pub enum FlashEvent {
    WaitingForDevice,
    DeviceCheckDiagnostic {
        stage: String,
        level: String,
        message: String,
    },
    GsiStatus {
        status: String,
    },
    PlanBuilt {
        actions: usize,
        total_bytes: u64,
    },
    PreparingImage {
        partition: String,
    },
    Flashing {
        partition: String,
        bytes: u64,
        total: u64,
        speed_bps: u64,
    },
    Simulating {
        partition: String,
        action: String,
        bytes: u64,
        total: u64,
        speed_bps: u64,
    },
    PartitionComplete {
        partition: String,
    },
    PartitionSkipped {
        partition: String,
        reason: String,
    },
    PartitionFailed {
        partition: String,
        error: String,
    },
    Erasing {
        partition: String,
    },
    EraseComplete {
        partition: String,
    },
    Overall {
        bytes: u64,
        total: u64,
    },
    Complete {
        summary: FlashSummaryDto,
    },
    Cancelled {
        message: String,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum ForceFastbootEvent {
    Started { session_id: u64 },
    WaitingForPreloader { session_id: u64 },
    Complete { session_id: u64 },
    Cancelled { session_id: u64 },
    Error { session_id: u64, message: String },
}

fn lock_device(
    state: &AppState,
) -> Result<std::sync::MutexGuard<'_, Option<FastbootDevice>>, String> {
    match state.device.lock() {
        Ok(guard) => Ok(guard),
        Err(poisoned) => {
            warn!("device lock was poisoned, recovering");
            Ok(poisoned.into_inner())
        }
    }
}

fn new_device_cache() -> DeviceCache {
    Mutex::new(None)
}

fn lock_flash_plans(state: &AppState) -> Result<std::sync::MutexGuard<'_, StoredPlans>, String> {
    match state.flash_plans.lock() {
        Ok(guard) => Ok(guard),
        Err(poisoned) => {
            warn!("flash_plans lock was poisoned, recovering");
            Ok(poisoned.into_inner())
        }
    }
}

fn lock_force_fastboot(
    state: &AppState,
) -> Result<std::sync::MutexGuard<'_, ForceFastbootState>, String> {
    match state.force_fastboot.lock() {
        Ok(guard) => Ok(guard),
        Err(poisoned) => {
            warn!("force_fastboot lock was poisoned, recovering");
            Ok(poisoned.into_inner())
        }
    }
}

fn try_begin_flash(state: &AppState) -> Result<(), String> {
    if state.flash_in_progress.swap(true, Ordering::Acquire) {
        return Err("a flash operation is already in progress".to_string());
    }
    Ok(())
}

fn end_flash(state: &AppState) {
    state.flash_in_progress.store(false, Ordering::Release);
}

struct FlashGuard<'a>(&'a AppState);

impl<'a> FlashGuard<'a> {
    fn new(state: &'a AppState) -> Result<Self, String> {
        try_begin_flash(state)?;
        Ok(Self(state))
    }
}

impl Drop for FlashGuard<'_> {
    fn drop(&mut self) {
        end_flash(self.0);
    }
}

fn take_device(state: &AppState) -> Result<Option<FastbootDevice>, String> {
    Ok(lock_device(state)?.take())
}

fn put_device(state: &AppState, dev: FastbootDevice) -> Result<(), String> {
    *lock_device(state)? = Some(dev);
    Ok(())
}

fn invalidate_cached_device(state: &AppState) -> Result<(), String> {
    let _ = take_device(state)?;
    Ok(())
}

async fn take_or_connect_device(state: &AppState) -> Result<FastbootDevice, String> {
    match take_device(state)? {
        Some(dev) => Ok(dev),
        None => pawflash::connect_fastboot()
            .await
            .map_err(|e| format!("connect: {e}")),
    }
}

fn session_policy_for_flash_run() -> DeviceSessionPolicy {
    DeviceSessionPolicy::Fresh
}

fn session_policy_for_read_only_command() -> DeviceSessionPolicy {
    if cfg!(windows) {
        DeviceSessionPolicy::Fresh
    } else {
        DeviceSessionPolicy::ReuseCached
    }
}

fn session_policy_for_mutating_command() -> DeviceSessionPolicy {
    DeviceSessionPolicy::Fresh
}

fn describe_fastboot_probe_failure(failure: &FastbootProbeFailure) -> String {
    match failure {
        FastbootProbeFailure::NoFastbootInterface => format!(
            "No fastboot device detected. {WINDOWS_FASTBOOTD_DRIVER_HINT}"
        ),
        FastbootProbeFailure::OpenFailed(error) => format!(
            "Fastboot device detected but could not be opened: {error}. {WINDOWS_FASTBOOTD_DRIVER_HINT}"
        ),
        FastbootProbeFailure::ReadVariablesFailed(error) => format!(
            "Fastboot device opened but did not respond to fastboot variables: {error}. {WINDOWS_FASTBOOTD_DRIVER_HINT}"
        ),
    }
}

fn build_device_check_diagnostic(
    stage: &str,
    level: &str,
    message: impl Into<String>,
) -> FlashEvent {
    FlashEvent::DeviceCheckDiagnostic {
        stage: stage.to_string(),
        level: level.to_string(),
        message: message.into(),
    }
}

fn emit_device_check_diagnostic(
    app: &tauri::AppHandle,
    stage: &str,
    level: &str,
    message: impl Into<String>,
) -> Result<(), String> {
    app.emit(
        "flash-progress",
        build_device_check_diagnostic(stage, level, message),
    )
    .map_err(|e| format!("emit: {e}"))
}

#[cfg(test)]
fn describe_probe_stage(stage: &str) -> &str {
    match stage {
        "using_cached_device" => "Using cached fastboot device",
        "enumerating" => "Enumerating fastboot devices",
        "candidate_found" => "Found fastboot interface candidate",
        "open_ok" => "Opened fastboot interface",
        "open_failed" => "Opening fastboot interface failed",
        "no_interface_yet" => "No fastboot interface detected yet",
        "retrying" => "Waiting before next device probe",
        "reading_vars" => "Reading fastboot variables",
        "read_vars_ok" => "Read fastboot variables",
        "read_vars_failed" => "Reading fastboot variables failed",
        "mode_detected" => "Detected fastboot mode",
        "refreshing_connection" => "Refreshing fastboot connection",
        "probe_failed" => "Fastboot probe failed",
        _ => stage,
    }
}

fn normalize_power_off_error(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("unknown command")
        || lower.contains("unsupported command")
        || lower.contains("not supported")
        || lower.contains("not support")
    {
        return POWER_OFF_UNSUPPORTED_MESSAGE.to_string();
    }
    message.to_string()
}

fn emit_flash_error(app: &tauri::AppHandle, message: impl Into<String>) -> String {
    let message = message.into();
    if message == CANCELLED_MESSAGE {
        let _ = app.emit(
            "flash-progress",
            FlashEvent::Cancelled {
                message: message.clone(),
            },
        );
    } else {
        let _ = app.emit(
            "flash-progress",
            FlashEvent::Error {
                message: message.clone(),
            },
        );
    }
    message
}

fn store_flash_plan(state: &AppState, plan: FlashPlan) -> Result<u64, String> {
    let mut stored = lock_flash_plans(state)?;
    let plan_id = stored.next_id;
    stored.next_id = stored.next_id.saturating_add(1);
    stored.plans.insert(plan_id, plan);

    if stored.plans.len() > 16 {
        let min_kept = plan_id.saturating_sub(15);
        stored
            .plans
            .retain(|existing_id, _| *existing_id >= min_kept);
    }

    Ok(plan_id)
}

fn load_flash_plan(state: &AppState, plan_id: u64) -> Result<Option<FlashPlan>, String> {
    Ok(lock_flash_plans(state)?.plans.get(&plan_id).cloned())
}

fn filter_actions<'a>(
    plan: &'a FlashPlan,
    partitions: &[String],
) -> Vec<&'a mtk_scatter_parser::FlashAction> {
    if partitions.is_empty() {
        return plan.actions.iter().collect();
    }

    plan.actions
        .iter()
        .filter(|action| partitions.contains(&action.partition))
        .collect()
}

fn plan_requires_connected_device(plan: &FlashPlan) -> bool {
    !matches!(plan.mode.as_str(), "dry_run" | "dry-run")
}

fn total_bytes_for_actions(actions: &[&mtk_scatter_parser::FlashAction]) -> u64 {
    actions
        .iter()
        .map(|action| u64::try_from(action.size).unwrap_or(0))
        .sum()
}

fn emit_plan_built(
    app: &tauri::AppHandle,
    action_count: usize,
    total_bytes: u64,
) -> Result<(), String> {
    app.emit(
        "flash-progress",
        FlashEvent::PlanBuilt {
            actions: action_count,
            total_bytes,
        },
    )
    .map_err(|e| format!("emit: {e}"))
}

fn update_overall_progress(
    completed_before: u64,
    current_bytes: u64,
    total_bytes: u64,
) -> (u64, u64) {
    (
        completed_before
            .saturating_add(current_bytes)
            .min(total_bytes),
        total_bytes,
    )
}

fn emit_overall_progress(
    app: &tauri::AppHandle,
    completed_before: u64,
    current_bytes: u64,
    total_bytes: u64,
) -> Result<(), String> {
    let (bytes, total) = update_overall_progress(completed_before, current_bytes, total_bytes);
    app.emit("flash-progress", FlashEvent::Overall { bytes, total })
        .map_err(|e| format!("emit: {e}"))
}

fn begin_flash_run(state: &AppState) -> FlashRunControl {
    state
        .flash_control
        .cancel_requested
        .store(false, Ordering::SeqCst);
    state.flash_control.clone()
}

fn request_cancel(state: &AppState) {
    state
        .flash_control
        .cancel_requested
        .store(true, Ordering::SeqCst);
}

fn start_force_fastboot_session(state: &AppState) -> Result<u64, String> {
    let mut force = lock_force_fastboot(state)?;
    let session_id = force.next_session_id.max(1);
    force.next_session_id = session_id.saturating_add(1);
    force.active_session_id = Some(session_id);
    Ok(session_id)
}

fn cancel_force_fastboot_session(state: &AppState, session_id: u64) -> bool {
    lock_force_fastboot(state)
        .map(|mut force| {
            if force.active_session_id == Some(session_id) {
                force.active_session_id = None;
                true
            } else {
                false
            }
        })
        .unwrap_or_else(|e| {
            warn!(error = %e, "force-fastboot cancel_session lock failed");
            false
        })
}

fn force_fastboot_session_is_active(state: &AppState, session_id: u64) -> bool {
    lock_force_fastboot(state)
        .map(|force| force.active_session_id == Some(session_id))
        .unwrap_or_else(|e| {
            warn!(error = %e, "force-fastboot session_is_active lock failed");
            false
        })
}

fn emit_force_fastboot_event(
    app: &tauri::AppHandle,
    event: ForceFastbootEvent,
) -> Result<(), String> {
    app.emit("force-fastboot-progress", event)
        .map_err(|e| format!("emit: {e}"))
}

fn ensure_not_cancelled(control: &FlashRunControl) -> Result<(), String> {
    if control.cancel_requested.load(Ordering::SeqCst) {
        Err(CANCELLED_MESSAGE.to_string())
    } else {
        Ok(())
    }
}

async fn wait_for_cancel(control: &FlashRunControl) -> String {
    loop {
        if control.cancel_requested.load(Ordering::SeqCst) {
            return CANCELLED_MESSAGE.to_string();
        }
        sleep(Duration::from_millis(50)).await;
    }
}

async fn ensure_device_with_policy(
    state: &AppState,
    app: &tauri::AppHandle,
    control: &FlashRunControl,
    policy: DeviceSessionPolicy,
) -> Result<FastbootDevice, String> {
    match policy {
        DeviceSessionPolicy::ReuseCached => match take_device(state)? {
            Some(dev) => Ok(dev),
            None => connect_device_for_run(app, control).await,
        },
        DeviceSessionPolicy::Fresh => {
            invalidate_cached_device(state)?;
            connect_device_for_run(app, control).await
        }
    }
}

async fn connect_device_for_run(
    app: &tauri::AppHandle,
    control: &FlashRunControl,
) -> Result<FastbootDevice, String> {
    app.emit("flash-progress", FlashEvent::WaitingForDevice)
        .map_err(|e| format!("emit: {e}"))?;
    tokio::select! {
        device = pawflash::connect_fastboot() => device.map_err(|e| format!("connect: {e}")),
        cancelled = wait_for_cancel(control) => Err(cancelled),
    }
}

async fn connect_device_with_policy(
    state: &AppState,
    policy: DeviceSessionPolicy,
) -> Result<FastbootDevice, String> {
    match policy {
        DeviceSessionPolicy::ReuseCached => take_or_connect_device(state).await,
        DeviceSessionPolicy::Fresh => {
            invalidate_cached_device(state)?;
            pawflash::connect_fastboot()
                .await
                .map_err(|e| format!("connect: {e}"))
        }
    }
}

async fn connect_fastboot_for_device_check(
    app: &tauri::AppHandle,
) -> Result<FastbootDevice, FastbootProbeFailure> {
    let deadline = Instant::now() + Duration::from_millis(DEVICE_CHECK_TIMEOUT_MS);
    let mut attempt = 0_u64;
    let mut errors = Vec::new();

    loop {
        attempt = attempt.saturating_add(1);
        let device = open_fastboot_with_observer(|event| {
            let level = match event.level {
                ProbeLogLevel::Info => "info",
                ProbeLogLevel::Warning => "warning",
                ProbeLogLevel::Error => "error",
            };
            let backend = match event.backend {
                BackendKind::Nusb => "nusb",
                #[cfg(windows)]
                BackendKind::AdbWinApi => "adbwinapi",
            };
            let _ = emit_device_check_diagnostic(
                app,
                event.stage,
                level,
                format!("Attempt {attempt}: backend={backend} {}", event.message),
            );
        })
        .await;
        match device {
            Ok(device) => return Ok(device),
            Err(error) => errors.push(error.to_string()),
        }

        if Instant::now() >= deadline {
            return Err(if let Some(error) = errors.pop() {
                FastbootProbeFailure::OpenFailed(error)
            } else {
                FastbootProbeFailure::NoFastbootInterface
            });
        }

        let _ = emit_device_check_diagnostic(
            app,
            "retrying",
            "info",
            format!("Attempt {attempt}: retrying in {DEVICE_RETRY_DELAY_MS} ms"),
        );
        sleep(Duration::from_millis(DEVICE_RETRY_DELAY_MS)).await;
    }
}

async fn connect_or_reconnect_device_for_check(
    state: &AppState,
    app: &tauri::AppHandle,
) -> Result<FastbootDevice, String> {
    if let Some(dev) = take_device(state)? {
        let _ = emit_device_check_diagnostic(
            app,
            "using_cached_device",
            "info",
            "Reusing cached fastboot session",
        );
        return Ok(dev);
    }

    connect_fastboot_for_device_check(app)
        .await
        .map_err(|failure| {
            let message = describe_fastboot_probe_failure(&failure);
            let _ = emit_device_check_diagnostic(app, "probe_failed", "error", &message);
            message
        })
}

async fn read_device_info_with_diagnostics(
    dev: &mut FastbootDevice,
    app: &tauri::AppHandle,
) -> Result<DeviceInfo, String> {
    let _ = emit_device_check_diagnostic(app, "reading_vars", "info", "Reading device variables");
    let info = read_device_info(dev).await?;
    let _ = emit_device_check_diagnostic(
        app,
        "read_vars_ok",
        "info",
        format!("Read {} fastboot variables", info.all_vars.len()),
    );
    let mode = pawflash::gsi::detect_fastboot_mode(&info.all_vars);
    let _ = emit_device_check_diagnostic(
        app,
        "mode_detected",
        "info",
        format!("Device reported mode={}", mode.as_str()),
    );
    Ok(info)
}

async fn check_device_with_diagnostics(
    state: &AppState,
    app: &tauri::AppHandle,
) -> Result<DeviceInfo, String> {
    let mut dev = connect_or_reconnect_device_for_check(state, app).await?;

    match read_device_info_with_diagnostics(&mut dev, app).await {
        Ok(info) => {
            put_device(state, dev)?;
            Ok(info)
        }
        Err(error) => {
            debug!(error = %error, "cached-or-fresh device read failed");
            let _ = emit_device_check_diagnostic(
                app,
                "read_vars_failed",
                "warning",
                format!("Cached device read failed: {error}"),
            );
            invalidate_cached_device(state)?;
            let _ = emit_device_check_diagnostic(
                app,
                "refreshing_connection",
                "info",
                "Discarding cached device and probing again",
            );
            let mut fresh = connect_fastboot_for_device_check(app)
                .await
                .map_err(|failure| {
                    let message = describe_fastboot_probe_failure(&failure);
                    let _ = emit_device_check_diagnostic(app, "probe_failed", "error", &message);
                    message
                })?;
            let info = read_device_info_with_diagnostics(&mut fresh, app)
                .await
                .map_err(|error| {
                    let message = describe_fastboot_probe_failure(
                        &FastbootProbeFailure::ReadVariablesFailed(error),
                    );
                    let _ =
                        emit_device_check_diagnostic(app, "read_vars_failed", "error", &message);
                    message
                })?;
            put_device(state, fresh)?;
            Ok(info)
        }
    }
}

struct FlashProgressContext<'a> {
    dev: &'a mut FastbootDevice,
    app: &'a tauri::AppHandle,
    summary: &'a mut FlashSummaryDto,
    control: &'a FlashRunControl,
    max_download_size: u32,
    overall_total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartitionFlashOutcome {
    Completed,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartitionFlashFailureDisposition {
    Skip,
    Fatal,
}

fn partition_flash_failure_disposition(error: &anyhow::Error) -> PartitionFlashFailureDisposition {
    if pawflash::flash::should_skip_failed_partition_error(error) {
        PartitionFlashFailureDisposition::Skip
    } else {
        PartitionFlashFailureDisposition::Fatal
    }
}

impl<'a> FlashProgressContext<'a> {
    async fn flash_partition(
        &mut self,
        partition: &str,
        image_path: &Path,
        bytes: u64,
        completed_before: u64,
        allow_skip_failed_partition: bool,
    ) -> Result<PartitionFlashOutcome, String> {
        ensure_not_cancelled(self.control)?;
        self.app
            .emit(
                "flash-progress",
                FlashEvent::PreparingImage {
                    partition: partition.to_string(),
                },
            )
            .map_err(|e| format!("emit: {e}"))?;

        emit_overall_progress(self.app, completed_before, 0, self.overall_total)?;

        let result = self
            .flash_one_partition_evented(partition, image_path, bytes, completed_before)
            .await;

        match result {
            Ok(()) => {
                self.summary.flash_count += 1;
                emit_overall_progress(self.app, completed_before, bytes, self.overall_total)?;
                self.app
                    .emit(
                        "flash-progress",
                        FlashEvent::PartitionComplete {
                            partition: partition.to_string(),
                        },
                    )
                    .map_err(|e| format!("emit: {e}"))?;
                Ok(PartitionFlashOutcome::Completed)
            }
            Err(error) => match (
                allow_skip_failed_partition,
                partition_flash_failure_disposition(&error),
            ) {
                (true, PartitionFlashFailureDisposition::Skip) => {
                    let reason = format!("{error:#}");
                    self.summary.skipped_count += 1;
                    emit_overall_progress(self.app, completed_before, bytes, self.overall_total)?;
                    self.app
                        .emit(
                            "flash-progress",
                            FlashEvent::PartitionSkipped {
                                partition: partition.to_string(),
                                reason: reason.clone(),
                            },
                        )
                        .map_err(|e| format!("emit: {e}"))?;
                    Ok(PartitionFlashOutcome::Skipped)
                }
                _ => {
                    let msg = format!("{error:#}");
                    self.app
                        .emit(
                            "flash-progress",
                            FlashEvent::PartitionFailed {
                                partition: partition.to_string(),
                                error: msg.clone(),
                            },
                        )
                        .map_err(|e| format!("emit: {e}"))?;
                    Err(msg)
                }
            },
        }
    }

    async fn erase_partition(
        &mut self,
        partition: &str,
        bytes: u64,
        completed_before: u64,
    ) -> Result<(), String> {
        ensure_not_cancelled(self.control)?;
        self.app
            .emit(
                "flash-progress",
                FlashEvent::Erasing {
                    partition: partition.to_string(),
                },
            )
            .map_err(|e| format!("emit: {e}"))?;
        emit_overall_progress(self.app, completed_before, 0, self.overall_total)?;

        match pawflash::erase_one_partition(self.dev, partition).await {
            Ok(()) => {
                self.summary.wipe_count += 1;
                emit_overall_progress(self.app, completed_before, bytes, self.overall_total)?;
                self.app
                    .emit(
                        "flash-progress",
                        FlashEvent::EraseComplete {
                            partition: partition.to_string(),
                        },
                    )
                    .map_err(|e| format!("emit: {e}"))?;
                Ok(())
            }
            Err(e) => {
                let msg = format!("{e:#}");
                self.app
                    .emit(
                        "flash-progress",
                        FlashEvent::PartitionFailed {
                            partition: partition.to_string(),
                            error: msg.clone(),
                        },
                    )
                    .map_err(|e| format!("emit: {e}"))?;
                Err(msg)
            }
        }
    }

    async fn execute_plan_actions(
        &mut self,
        actions: &[&mtk_scatter_parser::FlashAction],
        image_overrides: &HashMap<String, String>,
    ) -> Result<(), String> {
        let mut completed_before = 0_u64;

        for action in actions {
            ensure_not_cancelled(self.control)?;
            let action_bytes = u64::try_from(action.size).unwrap_or(0);
            match action.action.as_str() {
                "flash" => {
                    let image_path = resolve_image_path_for_action(action, image_overrides)?;

                    let _outcome = self.flash_partition(
                        &action.partition,
                        &image_path,
                        action_bytes,
                        completed_before,
                        true,
                    )
                    .await?;
                }
                "wipe" => {
                    self.erase_partition(&action.partition, action_bytes, completed_before)
                        .await?;
                }
                other => {
                    return Err(format!("unsupported plan action: {other}"));
                }
            }
            completed_before = completed_before.saturating_add(action_bytes);
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
        let app = self.app.clone();
        let p = partition.to_string();
        let p2 = p.clone();
        let emit_partition = p.clone();
        let overall_total = self.overall_total;
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<(u64, u64)>();
        let callback_tx = progress_tx.clone();
        let emit_task = tokio::spawn(async move {
            while let Some((bytes, speed_bps)) = progress_rx.recv().await {
                app.emit(
                    "flash-progress",
                    FlashEvent::Flashing {
                        partition: emit_partition.clone(),
                        bytes,
                        total: total_bytes,
                        speed_bps,
                    },
                )
                .map_err(|e| format!("emit: {e}"))?;
                emit_overall_progress(&app, completed_before, bytes, overall_total)?;
            }
            Ok::<(), String>(())
        });
        let mut bytes_flashed: u64 = 0;
        let start = std::time::Instant::now();

        pawflash::flash_one_partition(self.dev, &p2, image, self.max_download_size, move |event| {
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
                let _ = callback_tx.send((bytes_flashed, speed_bps));
            }
        })
        .await
        ?;

        let emit_result = emit_task.await.map_err(anyhow::Error::from)?;
        emit_result.map_err(anyhow::Error::msg)?;
        Ok(())
    }
}

async fn erase_optional_partition_and_emit(
    dev: &mut FastbootDevice,
    app: &tauri::AppHandle,
    summary: &mut FlashSummaryDto,
    control: &FlashRunControl,
    partition: &str,
    completed_before: u64,
    overall_total: u64,
) -> Result<(), String> {
    ensure_not_cancelled(control)?;
    app.emit(
        "flash-progress",
        FlashEvent::Erasing {
            partition: partition.to_string(),
        },
    )
    .map_err(|e| format!("emit: {e}"))?;
    emit_overall_progress(app, completed_before, 0, overall_total)?;

    match erase_optional_partition(dev, partition)
        .await
        .map_err(|e| format!("erase {partition}: {e}"))?
    {
        OptionalEraseOutcome::Erased => {
            summary.wipe_count += 1;
            emit_overall_progress(app, completed_before, 1, overall_total)?;
            app.emit(
                "flash-progress",
                FlashEvent::EraseComplete {
                    partition: partition.to_string(),
                },
            )
            .map_err(|e| format!("emit: {e}"))?;
        }
        OptionalEraseOutcome::Skipped { reason } => {
            summary.skipped_count += 1;
            emit_overall_progress(app, completed_before, 1, overall_total)?;
            app.emit(
                "flash-progress",
                FlashEvent::PartitionSkipped {
                    partition: partition.to_string(),
                    reason,
                },
            )
            .map_err(|e| format!("emit: {e}"))?;
        }
    }

    Ok(())
}

#[tauri::command]
async fn connect_device(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<DeviceInfo, String> {
    check_device_with_diagnostics(state.inner(), &app).await
}

#[tauri::command]
async fn check_device(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<DeviceInfo, String> {
    check_device_with_diagnostics(state.inner(), &app).await
}

#[tauri::command]
async fn get_variable(state: tauri::State<'_, AppState>, var: String) -> Result<String, String> {
    let mut dev =
        connect_device_with_policy(&state, session_policy_for_read_only_command()).await?;
    let result = pawflash::read_variable(&mut dev, &var)
        .await
        .map_err(|e| format!("getvar: {e}"));
    let _ = put_device(&state, dev);
    result
}

#[tauri::command]
async fn get_all_variables(
    state: tauri::State<'_, AppState>,
) -> Result<HashMap<String, String>, String> {
    let mut dev =
        connect_device_with_policy(&state, session_policy_for_read_only_command()).await?;
    let result = pawflash::read_all_variables(&mut dev)
        .await
        .map_err(|e| format!("getvars: {e}"));
    let _ = put_device(&state, dev);
    result
}

async fn read_device_info(dev: &mut FastbootDevice) -> Result<DeviceInfo, String> {
    let vars = pawflash::read_all_variables(dev)
        .await
        .map_err(|e| format!("read vars: {e}"))?;
    Ok(DeviceInfo {
        serial: vars.get("serialno").cloned().unwrap_or_default(),
        product: vars.get("product").cloned().unwrap_or_default(),
        slot: vars
            .get("current-slot")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        secure: vars.get("secure").cloned().unwrap_or_default(),
        unlocked: vars.get("unlocked").cloned().unwrap_or_default(),
        version: vars.get("version-bootloader").cloned().unwrap_or_default(),
        all_vars: vars,
    })
}

#[cfg(test)]
fn normalize_slot(slot: Option<&String>) -> Option<String> {
    match slot.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if value == "a" || value == "b" => Some(value),
        _ => None,
    }
}

#[tauri::command]
async fn parse_scatter(
    state: tauri::State<'_, AppState>,
    path: String,
    mode: String,
    slot: Option<String>,
    include_preloader: bool,
) -> Result<ParseScatterResponseDto, String> {
    let request = parse_plan_request(&mode, slot.as_deref())?;
    let scatter = mtk_scatter_parser::parse_scatter(&path)
        .map_err(|e| format!("parse scatter metadata: {e}"))?;
    let plan = pawflash::build_flash_plan(
        &PathBuf::from(&path),
        request.mode,
        request.slot,
        include_preloader,
        &[],
        false,
    )
    .map_err(|e| format!("parse scatter: {e}"))?;
    let dto = plan_to_dto(&plan, scatter.chipset());
    let plan_id = store_flash_plan(&state, plan)?;
    Ok(ParseScatterResponseDto { plan_id, plan: dto })
}

#[tauri::command]
async fn start_flash(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    plan_id: u64,
    partitions: Vec<String>,
    image_overrides: HashMap<String, String>,
    reboot: bool,
) -> Result<FlashSummaryDto, String> {
    start_flash_inner(
        state,
        app.clone(),
        plan_id,
        partitions,
        image_overrides,
        reboot,
    )
    .await
    .map_err(|error| emit_flash_error(&app, error))
}

#[tauri::command]
async fn start_gsi_flash(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    image: String,
) -> Result<FlashSummaryDto, String> {
    start_gsi_flash_inner(state, app.clone(), image)
        .await
        .map_err(|error| emit_flash_error(&app, error))
}

#[tauri::command]
async fn cancel_flash(state: tauri::State<'_, AppState>) -> Result<(), String> {
    request_cancel(&state);
    Ok(())
}

#[tauri::command]
async fn start_force_fastboot(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<ForceFastbootStartDto, String> {
    let session_id = start_force_fastboot_session(&state)?;
    emit_force_fastboot_event(&app, ForceFastbootEvent::Started { session_id })?;
    emit_force_fastboot_event(&app, ForceFastbootEvent::WaitingForPreloader { session_id })?;

    let app_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = pawflash::force_fastboot();
        let app_state = app_handle.state::<AppState>();
        if !force_fastboot_session_is_active(&app_state, session_id) {
            return;
        }

        let _ = cancel_force_fastboot_session(&app_state, session_id);
        match result {
            Ok(()) => {
                let _ = emit_force_fastboot_event(
                    &app_handle,
                    ForceFastbootEvent::Complete { session_id },
                );
            }
            Err(error) => {
                let _ = emit_force_fastboot_event(
                    &app_handle,
                    ForceFastbootEvent::Error {
                        session_id,
                        message: format!("force fastboot: {error}"),
                    },
                );
            }
        }
    });

    Ok(ForceFastbootStartDto { session_id })
}

#[tauri::command]
async fn cancel_force_fastboot(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    session_id: u64,
) -> Result<(), String> {
    if cancel_force_fastboot_session(&state, session_id) {
        emit_force_fastboot_event(&app, ForceFastbootEvent::Cancelled { session_id })?;
    }
    Ok(())
}

async fn start_flash_inner(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    plan_id: u64,
    partitions: Vec<String>,
    image_overrides: HashMap<String, String>,
    reboot: bool,
) -> Result<FlashSummaryDto, String> {
    let _guard = FlashGuard::new(&state)?;
    let control = begin_flash_run(&state);
    let plan = load_flash_plan(&state, plan_id)?
        .ok_or_else(|| format!("unknown flash plan: {plan_id}"))?;

    let filtered = filter_actions(&plan, &partitions);
    let total_bytes = total_bytes_for_actions(&filtered);

    emit_plan_built(&app, filtered.len(), total_bytes)?;
    emit_overall_progress(&app, 0, 0, total_bytes)?;

    let mut summary = FlashSummaryDto {
        flash_count: 0,
        wipe_count: 0,
        skipped_count: plan.summary.skipped_count,
        total_bytes,
    };

    if !plan_requires_connected_device(&plan) {
        simulate_dry_run_actions(&filtered, &app, &control, &mut summary, total_bytes).await?;

        app.emit(
            "flash-progress",
            FlashEvent::Complete {
                summary: summary.clone(),
            },
        )
        .map_err(|e| format!("emit: {e}"))?;

        return Ok(summary);
    }

    let mut dev =
        ensure_device_with_policy(&state, &app, &control, session_policy_for_flash_run()).await?;
    let vars = pawflash::read_all_variables(&mut dev)
        .await
        .map_err(|e| format!("read vars: {e}"))?;
    let max_download_size = resolve_max_download_size_from_vars(&vars)
        .map_err(|e| format!("max-download-size: {e}"))?;

    let mut flash = FlashProgressContext {
        dev: &mut dev,
        app: &app,
        summary: &mut summary,
        control: &control,
        max_download_size,
        overall_total: total_bytes,
    };
    flash.execute_plan_actions(&filtered, &image_overrides).await?;

    if reboot {
        pawflash::reboot_device(&mut dev)
            .await
            .map_err(|e| format!("reboot: {e}"))?;
    }
    drop(dev);

    app.emit(
        "flash-progress",
        FlashEvent::Complete {
            summary: summary.clone(),
        },
    )
    .map_err(|e| format!("emit: {e}"))?;

    Ok(summary)
}

async fn start_gsi_flash_inner(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    image: String,
) -> Result<FlashSummaryDto, String> {
    let _guard = FlashGuard::new(&state)?;
    let control = begin_flash_run(&state);
    prepare_for_gsi_worker_launch(&state).await?;
    gsi_worker::run_gsi_worker_and_emit(&state, &app, &control, image).await
}

async fn prepare_for_gsi_worker_launch(state: &AppState) -> Result<(), String> {
    // The GSI worker opens its own fresh USB session in a child process.
    // Release any cached parent-process handle first so Windows can reopen it reliably.
    invalidate_cached_device(state)?;
    Ok(())
}

async fn simulate_dry_run_actions(
    actions: &[&mtk_scatter_parser::FlashAction],
    app: &tauri::AppHandle,
    control: &FlashRunControl,
    summary: &mut FlashSummaryDto,
    overall_total: u64,
) -> Result<(), String> {
    let mut completed_before = 0_u64;

    for action in actions {
        ensure_not_cancelled(control)?;
        let partition = action.partition.clone();
        let total = u64::try_from(action.size).unwrap_or(0).max(1);
        let mut completed: u64 = 0;

        match action.action.as_str() {
            "flash" => {
                app.emit(
                    "flash-progress",
                    FlashEvent::PreparingImage {
                        partition: partition.clone(),
                    },
                )
                .map_err(|e| format!("emit: {e}"))?;

                for step in dry_run_steps(total, 1024) {
                    ensure_not_cancelled(control)?;
                    completed = completed.saturating_add(step.bytes);
                    emit_overall_progress(
                        app,
                        completed_before,
                        completed.min(total),
                        overall_total,
                    )?;
                    app.emit(
                        "flash-progress",
                        FlashEvent::Simulating {
                            partition: partition.clone(),
                            action: "flash".to_string(),
                            bytes: completed.min(total),
                            total,
                            speed_bps: 1024 * 1024 * 1024,
                        },
                    )
                    .map_err(|e| format!("emit: {e}"))?;
                    sleep(Duration::from_millis(20)).await;
                }

                summary.flash_count += 1;
                completed_before = completed_before.saturating_add(total);
                app.emit(
                    "flash-progress",
                    FlashEvent::PartitionComplete { partition },
                )
                .map_err(|e| format!("emit: {e}"))?;
            }
            "wipe" => {
                app.emit(
                    "flash-progress",
                    FlashEvent::Erasing {
                        partition: partition.clone(),
                    },
                )
                .map_err(|e| format!("emit: {e}"))?;

                for step in dry_run_steps(total, 1024) {
                    ensure_not_cancelled(control)?;
                    completed = completed.saturating_add(step.bytes);
                    emit_overall_progress(
                        app,
                        completed_before,
                        completed.min(total),
                        overall_total,
                    )?;
                    app.emit(
                        "flash-progress",
                        FlashEvent::Simulating {
                            partition: partition.clone(),
                            action: "wipe".to_string(),
                            bytes: completed.min(total),
                            total,
                            speed_bps: 1024 * 1024 * 1024,
                        },
                    )
                    .map_err(|e| format!("emit: {e}"))?;
                    sleep(Duration::from_millis(20)).await;
                }

                summary.wipe_count += 1;
                completed_before = completed_before.saturating_add(total);
                app.emit("flash-progress", FlashEvent::EraseComplete { partition })
                    .map_err(|e| format!("emit: {e}"))?;
            }
            other => return Err(format!("unsupported plan action: {other}")),
        }
    }

    Ok(())
}

#[tauri::command]
async fn manual_flash(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    partition: String,
    image: String,
    slot: Option<String>,
) -> Result<FlashSummaryDto, String> {
    manual_flash_inner(state, app.clone(), partition, image, slot)
        .await
        .map_err(|error| emit_flash_error(&app, error))
}

async fn manual_flash_inner(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    partition: String,
    image: String,
    slot: Option<String>,
) -> Result<FlashSummaryDto, String> {
    let slot = parse_slot(slot.as_deref());
    let actions =
        pawflash::manual::manual_flash_actions(&partition, PathBuf::from(&image), slot)
            .map_err(|e| format!("manual flash: {e}"))?;
    execute_manual_actions(state, app, &actions).await
}

#[tauri::command]
async fn disable_vbmeta(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<FlashSummaryDto, String> {
    disable_vbmeta_inner(state, app.clone())
        .await
        .map_err(|error| emit_flash_error(&app, error))
}

async fn disable_vbmeta_inner(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<FlashSummaryDto, String> {
    let empty_vbmeta = standalone_disable_vbmeta_path().map_err(|e| format!("vbmeta path: {e}"))?;
    let actions =
        disable_vbmeta_actions(&empty_vbmeta).map_err(|e| format!("vbmeta actions: {e}"))?;
    execute_manual_actions(state, app, &actions).await
}

#[tauri::command]
async fn format_userdata(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    erase_fallback: bool,
) -> Result<FlashSummaryDto, String> {
    format_userdata_inner(state, app.clone(), erase_fallback)
        .await
        .map_err(|error| emit_flash_error(&app, error))
}

#[tauri::command]
async fn wipe_data(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    no_metadata: bool,
    no_cache: bool,
    erase_fallback: bool,
) -> Result<FlashSummaryDto, String> {
    wipe_data_inner(state, app.clone(), no_metadata, no_cache, erase_fallback)
        .await
        .map_err(|error| emit_flash_error(&app, error))
}

fn resolve_format_tools(app: &tauri::AppHandle) -> Result<FormatTools, String> {
    let bundled = app
        .path()
        .resolve(
            "../../pawflash/assets/bin",
            BaseDirectory::Resource,
        )
        .ok();
    let dev =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../pawflash/assets/bin");

    let root = bundled.filter(|path| path.exists()).unwrap_or(dev);
    let platform = format_tools_platform()?;
    Ok(build_format_tools(root, platform))
}

async fn format_userdata_inner(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    erase_fallback: bool,
) -> Result<FlashSummaryDto, String> {
    let _guard = FlashGuard::new(&state)?;
    let control = begin_flash_run(&state);
    let mut dev =
        ensure_device_with_policy(&state, &app, &control, session_policy_for_flash_run()).await?;
    let tools = resolve_format_tools(&app)?;
    let info = detect_userdata(&mut dev)
        .await
        .map_err(|e| format!("detect userdata: {e}"))?;
    let max_download_size = info
        .max_download_size
        .ok_or_else(|| "detect userdata: missing max-download-size".to_string())
        .and_then(|value| {
            u32::try_from(value).map_err(|_| {
                "detect userdata: max-download-size exceeds supported range".to_string()
            })
        })?;

    app.emit(
        "flash-progress",
        FlashEvent::PreparingImage {
            partition: "userdata".to_string(),
        },
    )
    .map_err(|e| format!("emit: {e}"))?;

    let options = FormatUserdataOptions {
        erase_fallback,
        casefold: false,
    };
    let generated = generate_userdata_image(&tools, &info, &options);
    let total_bytes = match &generated {
        Ok(image) => image
            .image_len()
            .map_err(|e| format!("generated image: {e}"))?,
        Err(_) if erase_fallback => 1,
        Err(_) => 0,
    };

    emit_plan_built(&app, 1, total_bytes)?;
    emit_overall_progress(&app, 0, 0, total_bytes)?;

    let mut summary = FlashSummaryDto {
        flash_count: 0,
        wipe_count: 0,
        skipped_count: 0,
        total_bytes,
    };

    match generated {
        Ok(image) => {
            let mut flash = FlashProgressContext {
                dev: &mut dev,
                app: &app,
                summary: &mut summary,
                control: &control,
                max_download_size,
                overall_total: total_bytes,
            };
            flash
                .flash_partition("userdata", image.path(), total_bytes, 0, false)
                .await?;
        }
        Err(_error) if erase_fallback => {
            let mut flash = FlashProgressContext {
                dev: &mut dev,
                app: &app,
                summary: &mut summary,
                control: &control,
                max_download_size,
                overall_total: total_bytes.max(1),
            };
            flash
                .erase_partition("userdata", total_bytes.max(1), 0)
                .await?;
        }
        Err(error) => return Err(format!("generate userdata image: {error:#}")),
    }

    drop(dev);
    app.emit(
        "flash-progress",
        FlashEvent::Complete {
            summary: summary.clone(),
        },
    )
    .map_err(|e| format!("emit: {e}"))?;

    Ok(summary)
}

async fn wipe_data_inner(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    no_metadata: bool,
    no_cache: bool,
    erase_fallback: bool,
) -> Result<FlashSummaryDto, String> {
    let _guard = FlashGuard::new(&state)?;
    let control = begin_flash_run(&state);
    let mut dev =
        ensure_device_with_policy(&state, &app, &control, session_policy_for_flash_run()).await?;
    let tools = resolve_format_tools(&app)?;
    let info = detect_userdata(&mut dev)
        .await
        .map_err(|e| format!("detect userdata: {e}"))?;
    let max_download_size = info
        .max_download_size
        .ok_or_else(|| "detect userdata: missing max-download-size".to_string())
        .and_then(|value| {
            u32::try_from(value).map_err(|_| {
                "detect userdata: max-download-size exceeds supported range".to_string()
            })
        })?;

    app.emit(
        "flash-progress",
        FlashEvent::PreparingImage {
            partition: "userdata".to_string(),
        },
    )
    .map_err(|e| format!("emit: {e}"))?;

    let format_options = FormatUserdataOptions {
        erase_fallback,
        casefold: false,
    };
    let generated = generate_userdata_image(&tools, &info, &format_options);
    let erase_steps = usize::from(!no_metadata) + usize::from(!no_cache);
    let base_bytes = match &generated {
        Ok(image) => image
            .image_len()
            .map_err(|e| format!("generated image: {e}"))?,
        Err(_) if erase_fallback => 1,
        Err(_) => 0,
    };
    let total_bytes = base_bytes + u64::try_from(erase_steps).unwrap_or(0);

    emit_plan_built(&app, 1 + erase_steps, total_bytes)?;
    emit_overall_progress(&app, 0, 0, total_bytes)?;

    let mut summary = FlashSummaryDto {
        flash_count: 0,
        wipe_count: 0,
        skipped_count: 0,
        total_bytes,
    };

    match generated {
        Ok(image) => {
            let mut flash = FlashProgressContext {
                dev: &mut dev,
                app: &app,
                summary: &mut summary,
                control: &control,
                max_download_size,
                overall_total: total_bytes.max(1),
            };
            flash
                .flash_partition("userdata", image.path(), base_bytes.max(1), 0, false)
                .await?;
        }
        Err(_error) if erase_fallback => {
            let mut flash = FlashProgressContext {
                dev: &mut dev,
                app: &app,
                summary: &mut summary,
                control: &control,
                max_download_size,
                overall_total: total_bytes.max(1),
            };
            flash
                .erase_partition("userdata", base_bytes.max(1), 0)
                .await?;
        }
        Err(error) => return Err(format!("generate userdata image: {error:#}")),
    }

    let mut completed_before = base_bytes.max(1);
    let wipe_options = WipeDataOptions {
        erase_metadata: !no_metadata,
        erase_cache: !no_cache,
        erase_fallback,
        casefold: false,
    };

    if wipe_options.erase_metadata {
        erase_optional_partition_and_emit(
            &mut dev,
            &app,
            &mut summary,
            &control,
            "metadata",
            completed_before,
            total_bytes.max(1),
        )
        .await?;
        completed_before = completed_before.saturating_add(1);
    }
    if wipe_options.erase_cache {
        erase_optional_partition_and_emit(
            &mut dev,
            &app,
            &mut summary,
            &control,
            "cache",
            completed_before,
            total_bytes.max(1),
        )
        .await?;
    }

    drop(dev);
    app.emit(
        "flash-progress",
        FlashEvent::Complete {
            summary: summary.clone(),
        },
    )
    .map_err(|e| format!("emit: {e}"))?;

    Ok(summary)
}

async fn execute_manual_actions(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    actions: &[pawflash::manual::ManualFlashAction],
) -> Result<FlashSummaryDto, String> {
    let _guard = FlashGuard::new(&state)?;
    let control = begin_flash_run(&state);
    let total_bytes: u64 = actions.iter().map(|a| a.size).sum();
    app.emit(
        "flash-progress",
        FlashEvent::PlanBuilt {
            actions: actions.len(),
            total_bytes,
        },
    )
    .map_err(|e| format!("emit: {e}"))?;
    emit_overall_progress(&app, 0, 0, total_bytes)?;

    let mut dev =
        ensure_device_with_policy(&state, &app, &control, session_policy_for_flash_run()).await?;
    let vars = pawflash::read_all_variables(&mut dev)
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

    execute_manual_flash(
        actions,
        &mut dev,
        max_download_size,
        &app,
        &control,
        &mut summary,
        total_bytes,
    )
    .await?;
    drop(dev);

    app.emit(
        "flash-progress",
        FlashEvent::Complete {
            summary: summary.clone(),
        },
    )
    .map_err(|e| format!("emit: {e}"))?;

    Ok(summary)
}

async fn execute_manual_flash(
    actions: &[pawflash::manual::ManualFlashAction],
    dev: &mut FastbootDevice,
    max_download_size: u32,
    app: &tauri::AppHandle,
    control: &FlashRunControl,
    summary: &mut FlashSummaryDto,
    overall_total: u64,
) -> Result<(), String> {
    let mut completed_before = 0_u64;
    for action in actions {
        ensure_not_cancelled(control)?;
        let mut flash = FlashProgressContext {
            dev,
            app,
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

#[tauri::command]
async fn set_active_slot(state: tauri::State<'_, AppState>, slot: String) -> Result<(), String> {
    let mut dev = connect_device_with_policy(&state, session_policy_for_mutating_command()).await?;
    let result = pawflash::set_fastboot_active_slot(&mut dev, &slot)
        .await
        .map_err(|e| format!("set active: {e}"));
    drop(dev);
    result
}

#[tauri::command]
async fn reboot_device(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut dev = connect_device_with_policy(&state, session_policy_for_mutating_command()).await?;
    let result = pawflash::reboot_device(&mut dev)
        .await
        .map_err(|e| format!("reboot: {e}"));
    // Device is gone after reboot — don't put it back
    drop(dev);
    result
}

#[tauri::command]
async fn reboot_bootloader(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut dev = connect_device_with_policy(&state, session_policy_for_mutating_command()).await?;
    let result = pawflash::reboot_device_bootloader(&mut dev)
        .await
        .map_err(|e| format!("reboot bootloader: {e}"));
    drop(dev);
    result
}

#[tauri::command]
async fn reboot_fastboot(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut dev = connect_device_with_policy(&state, session_policy_for_mutating_command()).await?;
    let result = pawflash::reboot_device_fastboot(&mut dev)
        .await
        .map_err(|e| format!("reboot fastboot: {e}"));
    drop(dev);
    result
}

#[tauri::command]
async fn reboot_recovery(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut dev = connect_device_with_policy(&state, session_policy_for_mutating_command()).await?;
    let result = dev
        .reboot_to("recovery")
        .await
        .map_err(|e| format!("reboot recovery: {e}"));
    drop(dev);
    result
}

#[tauri::command]
async fn power_off_device(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut dev = connect_device_with_policy(&state, session_policy_for_mutating_command()).await?;
    let result = pawflash::power_off_device(&mut dev)
        .await
        .map_err(|e| normalize_power_off_error(&format!("power off: {e}")));
    drop(dev);
    result
}

#[tauri::command]
async fn unlock_bootloader(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut dev = connect_device_with_policy(&state, session_policy_for_mutating_command()).await?;
    let result = pawflash::send_flashing_unlock(&mut dev)
        .await
        .map_err(|e| format!("unlock: {e}"));
    drop(dev);
    result
}

#[tauri::command]
async fn lock_bootloader(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut dev = connect_device_with_policy(&state, session_policy_for_mutating_command()).await?;
    let result = pawflash::send_flashing_lock(&mut dev)
        .await
        .map_err(|e| format!("lock: {e}"));
    drop(dev);
    result
}

struct ParsedPlanRequest {
    mode: FlashMode,
    slot: Option<SlotArg>,
}

fn parse_plan_request(mode: &str, slot: Option<&str>) -> Result<ParsedPlanRequest, String> {
    Ok(ParsedPlanRequest {
        mode: parse_flash_mode(mode)?,
        slot: parse_slot(slot),
    })
}

fn parse_flash_mode(mode: &str) -> Result<FlashMode, String> {
    match mode {
        "dry_run" => Ok(FlashMode::DryRun),
        "firmware_upgrade" => Ok(FlashMode::FirmwareUpgrade),
        "clean_flash" => Ok(FlashMode::CleanFlash),
        "selective" => Ok(FlashMode::Selective),
        other => Err(format!("unknown flash mode: {other}")),
    }
}

fn parse_slot(slot: Option<&str>) -> Option<SlotArg> {
    match slot {
        Some("a") => Some(SlotArg::A),
        Some("b") => Some(SlotArg::B),
        Some("all") => Some(SlotArg::All),
        _ => None,
    }
}

fn resolve_image_path_for_action(
    action: &mtk_scatter_parser::FlashAction,
    image_overrides: &HashMap<String, String>,
) -> Result<PathBuf, String> {
    if let Some(path) = image_overrides.get(&action.partition) {
        return Ok(PathBuf::from(path));
    }

    action
        .image_resolved_path()
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing image path for {}", action.partition))
}

fn normalize_storage_label(storage: &str, selected_layouts: &[String]) -> String {
    let selected = selected_layouts.join(" ").to_uppercase();
    if selected.contains("UFS") {
        return "UFS".to_string();
    }
    if selected.contains("EMMC") || selected.contains("MMC") {
        return "EMMC".to_string();
    }

    let upper = storage.to_uppercase();
    if upper.contains("UFS") {
        "UFS".to_string()
    } else if upper.contains("EMMC") || upper.contains("MMC") {
        "EMMC".to_string()
    } else {
        storage.to_string()
    }
}

fn display_safety_class(safety_class: &str) -> String {
    match safety_class {
        "firmware" => "firmware",
        "android_system" => "android_system",
        "wipe_only" => "wipe_only",
        "identity_or_calibration" => "identity_or_calibration",
        "dangerous" => "dangerous",
        "bootloader_critical" => "bootloader_critical",
        "boot_critical" => "boot_critical",
        "regional" => "regional",
        "unknown" => "other",
        other => other,
    }
    .to_string()
}

fn default_partition_selected(action: &mtk_scatter_parser::FlashAction) -> bool {
    if action.action != "flash" {
        return true;
    }

    matches!(action.image_exists(), Some(true))
}

fn plan_to_dto(plan: &FlashPlan, chipset: Option<String>) -> FlashPlanDto {
    let partitions = plan
        .actions
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let image_path = a.image_resolved_path().map(ToOwned::to_owned);
            let image_name = image_path.as_deref().and_then(|path| {
                PathBuf::from(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            });

            PartitionDto {
                index: i,
                action: a.action.clone(),
                partition: a.partition.clone(),
                size_human: a.size_human.clone(),
                size_bytes: u64::try_from(a.size).unwrap_or(0),
                safety_class: display_safety_class(&a.safety_class),
                source: a.reason.clone(),
                image_path,
                image_name,
                selected: default_partition_selected(a),
            }
        })
        .collect();

    FlashPlanDto {
        mode: plan.mode.clone(),
        storage: normalize_storage_label(&plan.storage_selection, &plan.selected_layouts),
        slot_policy: plan.slot_policy_effective.clone(),
        chipset,
        summary: FlashSummaryDto {
            flash_count: plan.summary.flash_count,
            wipe_count: plan.summary.wipe_count,
            skipped_count: plan.summary.skipped_count,
            total_bytes: plan
                .actions
                .iter()
                .map(|a| u64::try_from(a.size).unwrap_or(0))
                .sum(),
        },
        partitions,
        warnings: plan.warnings.clone(),
        errors: plan.errors.clone(),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();
    std::panic::set_hook(Box::new(|info| {
        warn!(panic = %info, "tauri panic");
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "<unknown>".to_string());
        warn!(location, "tauri panic location");
    }));

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            app.manage(AppState {
                device: new_device_cache(),
                flash_plans: Mutex::new(StoredPlans {
                    next_id: 1,
                    plans: HashMap::new(),
                }),
                flash_control: FlashRunControl::default(),
                force_fastboot: Mutex::new(ForceFastbootState::default()),
                flash_in_progress: AtomicBool::new(false),
            });
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.maximize();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect_device,
            check_device,
            get_variable,
            get_all_variables,
            parse_scatter,
            start_flash,
            start_gsi_flash,
            cancel_flash,
            start_force_fastboot,
            cancel_force_fastboot,
            manual_flash,
            disable_vbmeta,
            format_userdata,
            set_active_slot,
            reboot_device,
            reboot_bootloader,
            reboot_fastboot,
            reboot_recovery,
            power_off_device,
            unlock_bootloader,
            lock_bootloader,
            wipe_data,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| match event {
        tauri::RunEvent::ExitRequested { code, api, .. } => {
            debug!(?code, "ExitRequested");
            let _ = api;
            if let Some(window) = app_handle.get_webview_window("main") {
                debug!(visible = ?window.is_visible().ok(), "main window still present");
            } else {
                debug!("main window missing at ExitRequested");
            }
        }
        tauri::RunEvent::WindowEvent { label, event, .. } => match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                debug!(label, "WindowEvent CloseRequested");
                let _ = api;
            }
            tauri::WindowEvent::Destroyed => {
                debug!(label, "WindowEvent Destroyed");
            }
            tauri::WindowEvent::Focused(focused) => {
                debug!(label, focused, "WindowEvent Focused");
            }
            _ => {}
        },
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::{
        build_device_check_diagnostic, cancel_force_fastboot_session,
        describe_fastboot_probe_failure, describe_probe_stage, display_safety_class,
        filter_actions, load_flash_plan, new_device_cache, normalize_power_off_error,
        normalize_slot, normalize_storage_label, parse_flash_mode, parse_plan_request,
        partition_flash_failure_disposition, plan_requires_connected_device, plan_to_dto,
        prepare_for_gsi_worker_launch, resolve_image_path_for_action, session_policy_for_flash_run,
        session_policy_for_mutating_command, session_policy_for_read_only_command,
        start_force_fastboot_session, store_flash_plan, update_overall_progress, AppState,
        DeviceSessionPolicy, FastbootProbeFailure, FlashEvent, FlashRunControl, ForceFastbootState,
        PartitionFlashFailureDisposition, StoredPlans, DEVICE_CHECK_TIMEOUT_MS,
    };
    use fastboot_rs::{
        transport::nusb::NusbFastBootError, FastbootError, FastbootExecutionError,
        ImagePayloadError,
    };
    use pawflash::gsi::{
        detect_fastboot_mode as shared_detect_fastboot_mode, FastbootMode as SharedFastbootMode,
    };
    use pawflash::plan::slot_to_scatter;
    use mtk_scatter_parser::{FlashAction, FlashPlan, FlashPlanSummary};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::collections::HashMap;
    use std::ffi::OsStr;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    fn flash_action(partition: &str, action: &str) -> FlashAction {
        FlashAction {
            action: action.to_string(),
            partition: partition.to_string(),
            base_name: partition.to_string(),
            slot: None,
            layout: "DEFAULT".to_string(),
            region: "EMMC_USER".to_string(),
            start: 0,
            start_hex: "0x0".to_string(),
            size: 1024,
            size_hex: "0x400".to_string(),
            size_human: "1 KiB".to_string(),
            image: Some(json!({ "path": { "resolved_path": format!("/tmp/{partition}.img") } })),
            safety_class: "boot_critical".to_string(),
            reason: "test".to_string(),
            warnings: Vec::new(),
        }
    }

    fn flash_plan(actions: Vec<FlashAction>) -> FlashPlan {
        FlashPlan {
            mode: "dry-run".to_string(),
            storage_selection: "EMMC".to_string(),
            selected_layouts: vec!["DEFAULT".to_string()],
            slot_policy_requested: "none".to_string(),
            slot_policy_effective: "none".to_string(),
            firmware_dir: None,
            package_root: None,
            options: json!({}),
            summary: FlashPlanSummary {
                flash_count: actions.iter().filter(|a| a.action == "flash").count(),
                wipe_count: actions.iter().filter(|a| a.action == "wipe").count(),
                skipped_count: 0,
                missing_image_count: 0,
                oversized_image_count: 0,
                action_warning_count: 0,
                incomplete_slot_base_count: 0,
                warning_count: 0,
                error_count: 0,
            },
            actions,
            skipped: Vec::new(),
            incomplete_slots: BTreeMap::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn test_state() -> AppState {
        AppState {
            device: new_device_cache(),
            flash_plans: Mutex::new(StoredPlans {
                next_id: 1,
                plans: HashMap::new(),
            }),
            flash_control: FlashRunControl::default(),
            force_fastboot: Mutex::new(ForceFastbootState::default()),
            flash_in_progress: AtomicBool::new(false),
        }
    }

    fn fastboot_failed_error() -> anyhow::Error {
        anyhow::Error::new(FastbootExecutionError::Fastboot(FastbootError::Nusb(
            NusbFastBootError::FastbootFailed("bootloader rejected flash".to_string()),
        )))
    }

    fn non_skippable_error() -> anyhow::Error {
        anyhow::Error::new(FastbootExecutionError::Payload(ImagePayloadError::SizeTooLarge(1024)))
    }

    #[test]
    fn filter_actions_returns_all_actions_when_no_partitions_are_selected() {
        let plan = flash_plan(vec![
            flash_action("boot", "flash"),
            flash_action("userdata", "wipe"),
        ]);

        let filtered = filter_actions(&plan, &[]);

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].partition, "boot");
        assert_eq!(filtered[1].partition, "userdata");
    }

    #[test]
    fn filter_actions_only_keeps_selected_partitions() {
        let plan = flash_plan(vec![
            flash_action("boot", "flash"),
            flash_action("vendor", "flash"),
            flash_action("userdata", "wipe"),
        ]);

        let filtered = filter_actions(&plan, &["vendor".to_string()]);

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].partition, "vendor");
    }

    #[test]
    fn parse_flash_mode_rejects_unknown_modes() {
        let error = parse_flash_mode("not-a-real-mode").unwrap_err();
        assert_eq!(error, "unknown flash mode: not-a-real-mode");
    }

    #[test]
    fn dry_run_plan_does_not_require_a_connected_device() {
        let plan = flash_plan(vec![flash_action("boot", "flash")]);

        assert!(!plan_requires_connected_device(&plan));
    }

    #[test]
    fn underscored_dry_run_plan_does_not_require_a_connected_device() {
        let mut plan = flash_plan(vec![flash_action("boot", "flash")]);
        plan.mode = "dry_run".to_string();

        assert!(!plan_requires_connected_device(&plan));
    }

    #[test]
    fn resolve_format_tools_uses_current_platform_directory() {
        let platform = super::format_tools_platform().unwrap();
        let root = PathBuf::from("/tmp/format-bin");
        let tools = super::build_format_tools(root.clone(), platform);

        assert_eq!(tools.dir.parent(), Some(root.as_path()));
        assert_eq!(tools.dir.file_name(), Some(OsStr::new(platform)));
    }

    #[test]
    fn live_flash_plan_requires_a_connected_device() {
        let mut plan = flash_plan(vec![flash_action("boot", "flash")]);
        plan.mode = "clean_flash".to_string();

        assert!(plan_requires_connected_device(&plan));
    }

    #[test]
    fn flash_runs_always_use_fresh_device_sessions() {
        assert_eq!(session_policy_for_flash_run(), DeviceSessionPolicy::Fresh);
    }

    #[test]
    fn mutating_commands_always_use_fresh_device_sessions() {
        assert_eq!(
            session_policy_for_mutating_command(),
            DeviceSessionPolicy::Fresh
        );
    }

    #[test]
    fn read_only_commands_may_reuse_cached_device_sessions() {
        let expected = if cfg!(windows) {
            DeviceSessionPolicy::Fresh
        } else {
            DeviceSessionPolicy::ReuseCached
        };
        assert_eq!(session_policy_for_read_only_command(), expected);
    }

    #[test]
    fn stored_plan_ids_resolve_exact_plans() {
        let state = test_state();

        let dry_run_id =
            store_flash_plan(&state, flash_plan(vec![flash_action("boot", "flash")])).unwrap();
        let mut clean_flash = flash_plan(vec![flash_action("boot", "flash")]);
        clean_flash.mode = "clean_flash".to_string();
        let clean_flash_id = store_flash_plan(&state, clean_flash).unwrap();

        assert_eq!(
            load_flash_plan(&state, dry_run_id).unwrap().unwrap().mode,
            "dry-run"
        );
        assert_eq!(
            load_flash_plan(&state, clean_flash_id)
                .unwrap()
                .unwrap()
                .mode,
            "clean_flash"
        );
    }

    #[test]
    fn plan_to_dto_exposes_resolved_image_metadata() {
        let dto = plan_to_dto(
            &flash_plan(vec![flash_action("boot", "flash")]),
            Some("mt6789".to_string()),
        );
        let partition = &dto.partitions[0];

        assert_eq!(partition.image_path.as_deref(), Some("/tmp/boot.img"));
        assert_eq!(partition.image_name.as_deref(), Some("boot.img"));
        assert_eq!(dto.chipset.as_deref(), Some("mt6789"));
    }

    #[test]
    fn plan_to_dto_only_selects_flash_actions_with_existing_images_by_default() {
        let mut flash_with_existing = flash_action("boot", "flash");
        flash_with_existing.image = Some(json!({
            "path": {
                "resolved_path": "/tmp/boot.img",
                "exists": true
            }
        }));

        let mut flash_missing_image = flash_action("vendor", "flash");
        flash_missing_image.image = Some(json!({
            "path": {
                "resolved_path": "/tmp/vendor.img",
                "exists": false
            }
        }));

        let wipe_action = flash_action("userdata", "wipe");

        let dto = plan_to_dto(
            &flash_plan(vec![flash_with_existing, flash_missing_image, wipe_action]),
            None,
        );

        assert!(dto.partitions[0].selected);
        assert!(!dto.partitions[1].selected);
        assert!(dto.partitions[2].selected);
    }

    #[test]
    fn image_override_path_takes_precedence_for_flash_actions() {
        let action = flash_action("boot", "flash");
        let overrides =
            HashMap::from([("boot".to_string(), "/custom/boot_patched.img".to_string())]);

        let resolved = resolve_image_path_for_action(&action, &overrides).unwrap();

        assert_eq!(resolved, PathBuf::from("/custom/boot_patched.img"));
    }

    #[test]
    fn parse_plan_request_maps_all_slot_to_both_slot_policy() {
        let request = parse_plan_request("dry_run", Some("all")).unwrap();

        assert_eq!(
            slot_to_scatter(request.slot),
            mtk_scatter_parser::SlotPolicy::Both
        );
    }

    #[test]
    fn flash_run_control_marks_cancel_requested() {
        let control = FlashRunControl::default();

        assert!(!control.cancel_requested.load(Ordering::SeqCst));

        control.cancel_requested.store(true, Ordering::SeqCst);

        assert!(control.cancel_requested.load(Ordering::SeqCst));
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
            partition_flash_failure_disposition(&non_skippable_error()),
            PartitionFlashFailureDisposition::Fatal
        );
    }

    #[test]
    fn update_overall_progress_caps_bytes_at_total() {
        let (bytes, total) = update_overall_progress(2048, 1024, 2500);

        assert_eq!(bytes, 2500);
        assert_eq!(total, 2500);
    }

    #[test]
    fn normalize_storage_label_prefers_emmc_and_ufs() {
        assert_eq!(normalize_storage_label("auto", &[]), "auto");
        assert_eq!(normalize_storage_label("UFS", &[]), "UFS");
        assert_eq!(normalize_storage_label("EMMC", &[]), "EMMC");
        assert_eq!(normalize_storage_label("ufs_auto", &[]), "UFS");
        assert_eq!(normalize_storage_label("mmc_user", &[]), "EMMC");
        assert_eq!(normalize_storage_label("auto", &["UFS".to_string()]), "UFS");
    }

    #[test]
    fn normalize_slot_only_accepts_a_and_b() {
        assert_eq!(normalize_slot(Some(&"a".to_string())).as_deref(), Some("a"));
        assert_eq!(normalize_slot(Some(&"B".to_string())).as_deref(), Some("b"));
        assert_eq!(normalize_slot(Some(&"other".to_string())), None);
    }

    #[test]
    fn detect_fastboot_mode_treats_is_userspace_yes_as_fastbootd() {
        let vars = HashMap::from([("is-userspace".to_string(), "yes".to_string())]);

        assert_eq!(
            shared_detect_fastboot_mode(&vars),
            SharedFastbootMode::Fastbootd
        );
    }

    #[test]
    fn detect_fastboot_mode_treats_missing_or_non_yes_as_bootloader() {
        let missing = HashMap::new();
        let no = HashMap::from([("is-userspace".to_string(), "no".to_string())]);
        let upper = HashMap::from([("is-userspace".to_string(), "YES".to_string())]);

        assert_eq!(
            shared_detect_fastboot_mode(&missing),
            SharedFastbootMode::Bootloader
        );
        assert_eq!(
            shared_detect_fastboot_mode(&no),
            SharedFastbootMode::Bootloader
        );
        assert_eq!(
            shared_detect_fastboot_mode(&upper),
            SharedFastbootMode::Bootloader
        );
    }

    #[test]
    fn unknown_safety_class_displays_as_other() {
        assert_eq!(display_safety_class("unknown"), "other");
        assert_eq!(display_safety_class("boot_critical"), "boot_critical");
    }

    #[test]
    fn force_fastboot_sessions_are_replaced_and_cancellable() {
        let state = test_state();

        let first = start_force_fastboot_session(&state).unwrap();
        let second = start_force_fastboot_session(&state).unwrap();

        assert_eq!(first, 1);
        assert_eq!(second, 2);
        assert!(!cancel_force_fastboot_session(&state, first));
        assert!(cancel_force_fastboot_session(&state, second));
        assert!(!cancel_force_fastboot_session(&state, second));
    }

    #[tokio::test]
    async fn preparing_for_gsi_worker_launch_is_a_safe_no_op_without_cached_device() {
        let state = test_state();

        prepare_for_gsi_worker_launch(&state).await.unwrap();

        assert!(state.device.lock().unwrap().is_none());
    }

    #[test]
    fn device_check_timeout_is_two_minutes() {
        assert_eq!(DEVICE_CHECK_TIMEOUT_MS, 120_000);
    }

    #[test]
    fn fastboot_probe_failure_messages_include_windows_driver_hint() {
        let no_interface =
            describe_fastboot_probe_failure(&FastbootProbeFailure::NoFastbootInterface);
        let open_failed = describe_fastboot_probe_failure(&FastbootProbeFailure::OpenFailed(
            "access denied".to_string(),
        ));
        let read_failed = describe_fastboot_probe_failure(
            &FastbootProbeFailure::ReadVariablesFailed("read vars: transport stalled".to_string()),
        );

        assert!(no_interface.contains("No fastboot device detected"));
        assert!(no_interface.contains("Windows"));
        assert!(open_failed.contains("could not be opened"));
        assert!(open_failed.contains("access denied"));
        assert!(read_failed.contains("did not respond to fastboot variables"));
        assert!(read_failed.contains("transport stalled"));
    }

    #[test]
    fn device_check_diagnostic_events_capture_stage_and_level() {
        let event = build_device_check_diagnostic("read_vars_failed", "error", "transport stalled");

        assert_eq!(
            event,
            FlashEvent::DeviceCheckDiagnostic {
                stage: "read_vars_failed".to_string(),
                level: "error".to_string(),
                message: "transport stalled".to_string(),
            }
        );
    }

    #[test]
    fn probe_stage_labels_are_human_readable() {
        assert_eq!(
            describe_probe_stage("enumerating"),
            "Enumerating fastboot devices"
        );
        assert_eq!(
            describe_probe_stage("read_vars_failed"),
            "Reading fastboot variables failed"
        );
        assert_eq!(
            describe_probe_stage("mode_detected"),
            "Detected fastboot mode"
        );
        assert_eq!(describe_probe_stage("something_else"), "something_else");
    }

    #[test]
    fn normalize_power_off_error_maps_unsupported_command_failures() {
        let normalized = normalize_power_off_error(
            "power off: Fastboot client failure: unknown command powerdown",
        );

        assert_eq!(
            normalized,
            "Power off is not supported by this device in the current fastboot mode."
        );
    }
}
