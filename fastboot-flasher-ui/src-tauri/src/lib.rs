use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use fastboot_flasher::{
    self,
    cli::{FlashMode, SlotArg},
    format::{
        detect_userdata, erase_optional_partition, generate_userdata_image, FormatTools,
        FormatUserdataOptions, OptionalEraseOutcome, WipeDataOptions,
    },
    manual::{disable_vbmeta_actions, standalone_disable_vbmeta_path},
    progress::dry_run_steps,
    NusbFastBoot,
};
use fastboot_rs::FlashProgress;
use mtk_scatter_parser::FlashPlan;
use serde::Serialize;
use tauri::{path::BaseDirectory, Emitter, Manager};
use tokio::time::{sleep, Duration};

const CANCELLED_MESSAGE: &str = "cancelled by user";

struct AppState {
    device: Mutex<Option<NusbFastBoot>>,
    flash_plans: Mutex<StoredPlans>,
    flash_control: FlashRunControl,
    force_fastboot: Mutex<ForceFastbootState>,
}

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

#[derive(Clone, Serialize)]
pub struct FlashSummaryDto {
    flash_count: usize,
    wipe_count: usize,
    skipped_count: usize,
    total_bytes: u64,
}

#[derive(Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum FlashEvent {
    WaitingForDevice,
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

fn take_device(state: &AppState) -> Option<NusbFastBoot> {
    state.device.lock().unwrap().take()
}

fn put_device(state: &AppState, dev: NusbFastBoot) {
    *state.device.lock().unwrap() = Some(dev);
}

async fn take_or_connect_device(state: &AppState) -> Result<NusbFastBoot, String> {
    match take_device(state) {
        Some(dev) => Ok(dev),
        None => fastboot_flasher::connect_fastboot()
            .await
            .map_err(|e| format!("connect: {e}")),
    }
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

fn store_flash_plan(state: &AppState, plan: FlashPlan) -> u64 {
    let mut stored = state.flash_plans.lock().unwrap();
    let plan_id = stored.next_id;
    stored.next_id = stored.next_id.saturating_add(1);
    stored.plans.insert(plan_id, plan);

    if stored.plans.len() > 16 {
        let min_kept = plan_id.saturating_sub(15);
        stored
            .plans
            .retain(|existing_id, _| *existing_id >= min_kept);
    }

    plan_id
}

fn load_flash_plan(state: &AppState, plan_id: u64) -> Option<FlashPlan> {
    state
        .flash_plans
        .lock()
        .unwrap()
        .plans
        .get(&plan_id)
        .cloned()
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

fn start_force_fastboot_session(state: &AppState) -> u64 {
    let mut force = state.force_fastboot.lock().unwrap();
    let session_id = force.next_session_id.max(1);
    force.next_session_id = session_id.saturating_add(1);
    force.active_session_id = Some(session_id);
    session_id
}

fn cancel_force_fastboot_session(state: &AppState, session_id: u64) -> bool {
    let mut force = state.force_fastboot.lock().unwrap();
    if force.active_session_id == Some(session_id) {
        force.active_session_id = None;
        true
    } else {
        false
    }
}

fn force_fastboot_session_is_active(state: &AppState, session_id: u64) -> bool {
    state.force_fastboot.lock().unwrap().active_session_id == Some(session_id)
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

async fn ensure_device(
    state: &AppState,
    app: &tauri::AppHandle,
    control: &FlashRunControl,
) -> Result<NusbFastBoot, String> {
    match take_device(state) {
        Some(dev) => Ok(dev),
        None => {
            app.emit("flash-progress", FlashEvent::WaitingForDevice)
                .map_err(|e| format!("emit: {e}"))?;
            tokio::select! {
                device = fastboot_flasher::connect_fastboot() => {
                    device.map_err(|e| format!("connect: {e}"))
                }
                cancelled = wait_for_cancel(control) => Err(cancelled),
            }
        }
    }
}

async fn flash_partition_and_emit(
    dev: &mut NusbFastBoot,
    app: &tauri::AppHandle,
    summary: &mut FlashSummaryDto,
    control: &FlashRunControl,
    partition: &str,
    image_path: &std::path::Path,
    bytes: u64,
    completed_before: u64,
    overall_total: u64,
) -> Result<(), String> {
    ensure_not_cancelled(control)?;
    app.emit(
        "flash-progress",
        FlashEvent::PreparingImage {
            partition: partition.to_string(),
        },
    )
    .map_err(|e| format!("emit: {e}"))?;

    emit_overall_progress(app, completed_before, 0, overall_total)?;

    let result = flash_one_partition_evented(
        dev,
        partition,
        image_path,
        bytes,
        app,
        completed_before,
        overall_total,
    )
    .await;

    match result {
        Ok(()) => {
            summary.flash_count += 1;
            emit_overall_progress(app, completed_before, bytes, overall_total)?;
            app.emit(
                "flash-progress",
                FlashEvent::PartitionComplete {
                    partition: partition.to_string(),
                },
            )
            .map_err(|e| format!("emit: {e}"))?;
            Ok(())
        }
        Err(e) => {
            let msg = format!("{e:#}");
            app.emit(
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

async fn erase_partition_and_emit(
    dev: &mut NusbFastBoot,
    app: &tauri::AppHandle,
    summary: &mut FlashSummaryDto,
    control: &FlashRunControl,
    partition: &str,
    bytes: u64,
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

    match fastboot_flasher::erase_one_partition(dev, partition).await {
        Ok(()) => {
            summary.wipe_count += 1;
            emit_overall_progress(app, completed_before, bytes, overall_total)?;
            app.emit(
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
            app.emit(
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

async fn erase_optional_partition_and_emit(
    dev: &mut NusbFastBoot,
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
async fn connect_device(state: tauri::State<'_, AppState>) -> Result<DeviceInfo, String> {
    let mut dev = fastboot_flasher::connect_fastboot()
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let info = read_device_info(&mut dev).await?;
    put_device(&state, dev);
    Ok(info)
}

#[tauri::command]
async fn check_device(state: tauri::State<'_, AppState>) -> Result<DeviceInfo, String> {
    let mut dev = take_or_connect_device(&state).await?;
    let info = read_device_info(&mut dev).await?;
    put_device(&state, dev);
    Ok(info)
}

#[tauri::command]
async fn get_variable(state: tauri::State<'_, AppState>, var: String) -> Result<String, String> {
    let mut dev = take_or_connect_device(&state).await?;
    let result = fastboot_flasher::read_variable(&mut dev, &var)
        .await
        .map_err(|e| format!("getvar: {e}"));
    put_device(&state, dev);
    result
}

#[tauri::command]
async fn get_all_variables(
    state: tauri::State<'_, AppState>,
) -> Result<HashMap<String, String>, String> {
    let mut dev = take_or_connect_device(&state).await?;
    let result = fastboot_flasher::read_all_variables(&mut dev)
        .await
        .map_err(|e| format!("getvars: {e}"));
    put_device(&state, dev);
    result
}

async fn read_device_info(dev: &mut NusbFastBoot) -> Result<DeviceInfo, String> {
    let vars = fastboot_flasher::read_all_variables(dev)
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
    let plan = fastboot_flasher::build_flash_plan(
        &PathBuf::from(&path),
        request.mode,
        request.slot,
        include_preloader,
        Vec::new(),
        false,
    )
    .map_err(|e| format!("parse scatter: {e}"))?;
    let dto = plan_to_dto(
        &plan,
        resolve_chipset_label(scatter.platform, scatter.project),
    );
    let plan_id = store_flash_plan(&state, plan);
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
async fn cancel_flash(state: tauri::State<'_, AppState>) -> Result<(), String> {
    request_cancel(&state);
    Ok(())
}

#[tauri::command]
async fn start_force_fastboot(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<ForceFastbootStartDto, String> {
    let session_id = start_force_fastboot_session(&state);
    emit_force_fastboot_event(&app, ForceFastbootEvent::Started { session_id })?;
    emit_force_fastboot_event(&app, ForceFastbootEvent::WaitingForPreloader { session_id })?;

    let app_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = fastboot_flasher::force_fastboot();
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
    let control = begin_flash_run(&state);
    let plan =
        load_flash_plan(&state, plan_id).ok_or_else(|| format!("unknown flash plan: {plan_id}"))?;

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

    let mut dev = ensure_device(&state, &app, &control).await?;

    execute_plan_actions(
        &filtered,
        &image_overrides,
        &mut dev,
        &app,
        &control,
        &mut summary,
        total_bytes,
    )
    .await?;

    if reboot {
        fastboot_flasher::reboot_device(&mut dev)
            .await
            .map_err(|e| format!("reboot: {e}"))?;
        drop(dev);
    } else {
        put_device(&state, dev);
    }

    app.emit(
        "flash-progress",
        FlashEvent::Complete {
            summary: summary.clone(),
        },
    )
    .map_err(|e| format!("emit: {e}"))?;

    Ok(summary)
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

async fn execute_plan_actions(
    actions: &[&mtk_scatter_parser::FlashAction],
    image_overrides: &HashMap<String, String>,
    dev: &mut NusbFastBoot,
    app: &tauri::AppHandle,
    control: &FlashRunControl,
    summary: &mut FlashSummaryDto,
    overall_total: u64,
) -> Result<(), String> {
    let mut completed_before = 0_u64;

    for action in actions {
        ensure_not_cancelled(control)?;
        let action_bytes = u64::try_from(action.size).unwrap_or(0);
        match action.action.as_str() {
            "flash" => {
                let image_path = resolve_image_path_for_action(action, image_overrides)?;

                flash_partition_and_emit(
                    dev,
                    app,
                    summary,
                    control,
                    &action.partition,
                    &image_path,
                    action_bytes,
                    completed_before,
                    overall_total,
                )
                .await?;
            }
            "wipe" => {
                erase_partition_and_emit(
                    dev,
                    app,
                    summary,
                    control,
                    &action.partition,
                    action_bytes,
                    completed_before,
                    overall_total,
                )
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
        fastboot_flasher::manual::manual_flash_actions(&partition, &PathBuf::from(&image), slot)
            .map_err(|e| format!("manual flash: {e}"))?;
    execute_manual_actions(state, app, actions).await
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
    execute_manual_actions(state, app, actions).await
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
        .resolve("../../fastboot-flasher/assets/bin/linux", BaseDirectory::Resource)
        .ok();
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fastboot-flasher/assets/bin/linux");

    let dir = bundled.filter(|path| path.exists()).unwrap_or(dev);
    Ok(FormatTools {
        root: dir.clone(),
        mke2fs: dir.join("mke2fs"),
        make_f2fs: dir.join("make_f2fs"),
        make_f2fs_casefold: dir.join("make_f2fs_casefold"),
        mke2fs_conf: dir.join("mke2fs.conf"),
        dir,
    })
}

async fn format_userdata_inner(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    erase_fallback: bool,
) -> Result<FlashSummaryDto, String> {
    let control = begin_flash_run(&state);
    let mut dev = ensure_device(&state, &app, &control).await?;
    let tools = resolve_format_tools(&app)?;
    let info = detect_userdata(&mut dev)
        .await
        .map_err(|e| format!("detect userdata: {e}"))?;

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
            flash_partition_and_emit(
                &mut dev,
                &app,
                &mut summary,
                &control,
                "userdata",
                image.path(),
                total_bytes,
                0,
                total_bytes,
            )
            .await?;
        }
        Err(_error) if erase_fallback => {
            erase_partition_and_emit(
                &mut dev,
                &app,
                &mut summary,
                &control,
                "userdata",
                total_bytes.max(1),
                0,
                total_bytes.max(1),
            )
            .await?;
        }
        Err(error) => return Err(format!("generate userdata image: {error:#}")),
    }

    put_device(&state, dev);
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
    let control = begin_flash_run(&state);
    let mut dev = ensure_device(&state, &app, &control).await?;
    let tools = resolve_format_tools(&app)?;
    let info = detect_userdata(&mut dev)
        .await
        .map_err(|e| format!("detect userdata: {e}"))?;

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
            flash_partition_and_emit(
                &mut dev,
                &app,
                &mut summary,
                &control,
                "userdata",
                image.path(),
                base_bytes.max(1),
                0,
                total_bytes.max(1),
            )
            .await?;
        }
        Err(_error) if erase_fallback => {
            erase_partition_and_emit(
                &mut dev,
                &app,
                &mut summary,
                &control,
                "userdata",
                base_bytes.max(1),
                0,
                total_bytes.max(1),
            )
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

    put_device(&state, dev);
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
    actions: Vec<fastboot_flasher::manual::ManualFlashAction>,
) -> Result<FlashSummaryDto, String> {
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

    let mut dev = ensure_device(&state, &app, &control).await?;

    let mut summary = FlashSummaryDto {
        flash_count: 0,
        wipe_count: 0,
        skipped_count: 0,
        total_bytes,
    };

    execute_manual_flash(
        &actions,
        &mut dev,
        &app,
        &control,
        &mut summary,
        total_bytes,
    )
    .await?;
    put_device(&state, dev);

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
    actions: &[fastboot_flasher::manual::ManualFlashAction],
    dev: &mut NusbFastBoot,
    app: &tauri::AppHandle,
    control: &FlashRunControl,
    summary: &mut FlashSummaryDto,
    overall_total: u64,
) -> Result<(), String> {
    let mut completed_before = 0_u64;
    for action in actions {
        ensure_not_cancelled(control)?;
        flash_partition_and_emit(
            dev,
            app,
            summary,
            control,
            &action.partition,
            &action.image,
            action.size,
            completed_before,
            overall_total,
        )
        .await?;
        completed_before = completed_before.saturating_add(action.size);
    }
    Ok(())
}

async fn flash_one_partition_evented(
    dev: &mut NusbFastBoot,
    partition: &str,
    image: &std::path::Path,
    total_bytes: u64,
    app: &tauri::AppHandle,
    completed_before: u64,
    overall_total: u64,
) -> anyhow::Result<()> {
    let app = app.clone();
    let p = partition.to_string();
    let p2 = p.clone();
    let mut bytes_flashed: u64 = 0;
    let start = std::time::Instant::now();

    fastboot_flasher::flash_one_partition(dev, &p2, image, move |event| {
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
            let _ = app.emit(
                "flash-progress",
                FlashEvent::Flashing {
                    partition: p.clone(),
                    bytes: bytes_flashed,
                    total: total_bytes,
                    speed_bps,
                },
            );
            let _ = emit_overall_progress(&app, completed_before, bytes_flashed, overall_total);
        }
    })
    .await
}

#[tauri::command]
async fn set_active_slot(state: tauri::State<'_, AppState>, slot: String) -> Result<(), String> {
    let mut dev = take_or_connect_device(&state).await?;
    let result = fastboot_flasher::set_fastboot_active_slot(&mut dev, &slot)
        .await
        .map_err(|e| format!("set active: {e}"));
    put_device(&state, dev);
    result
}

#[tauri::command]
async fn reboot_device(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut dev = take_or_connect_device(&state).await?;
    let result = fastboot_flasher::reboot_device(&mut dev)
        .await
        .map_err(|e| format!("reboot: {e}"));
    // Device is gone after reboot — don't put it back
    drop(dev);
    Ok(result?)
}

#[tauri::command]
async fn reboot_bootloader(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut dev = take_or_connect_device(&state).await?;
    let result = fastboot_flasher::reboot_device_bootloader(&mut dev)
        .await
        .map_err(|e| format!("reboot bootloader: {e}"));
    drop(dev);
    Ok(result?)
}

#[tauri::command]
async fn reboot_recovery(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut dev = take_or_connect_device(&state).await?;
    let result = dev
        .reboot_to("recovery")
        .await
        .map_err(|e| format!("reboot recovery: {e}"));
    drop(dev);
    Ok(result?)
}

#[tauri::command]
async fn power_off_device(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut dev = take_or_connect_device(&state).await?;
    let result = fastboot_flasher::power_off_device(&mut dev)
        .await
        .map_err(|e| format!("power off: {e}"));
    drop(dev);
    Ok(result?)
}

#[tauri::command]
async fn unlock_bootloader(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut dev = take_or_connect_device(&state).await?;
    let result = fastboot_flasher::send_flashing_unlock(&mut dev)
        .await
        .map_err(|e| format!("unlock: {e}"));
    put_device(&state, dev);
    result
}

#[tauri::command]
async fn lock_bootloader(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut dev = take_or_connect_device(&state).await?;
    let result = fastboot_flasher::send_flashing_lock(&mut dev)
        .await
        .map_err(|e| format!("lock: {e}"));
    put_device(&state, dev);
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
                selected: true,
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

fn resolve_chipset_label(platform: Option<String>, project: Option<String>) -> Option<String> {
    platform
        .filter(|value| !value.trim().is_empty())
        .or_else(|| project.filter(|value| !value.trim().is_empty()))
        .or_else(|| Some("Unknown".to_string()))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            app.manage(AppState {
                device: Mutex::new(None),
                flash_plans: Mutex::new(StoredPlans {
                    next_id: 1,
                    plans: HashMap::new(),
                }),
                flash_control: FlashRunControl::default(),
                force_fastboot: Mutex::new(ForceFastbootState::default()),
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
            cancel_flash,
            start_force_fastboot,
            cancel_force_fastboot,
            manual_flash,
            disable_vbmeta,
            format_userdata,
            set_active_slot,
            reboot_device,
            reboot_bootloader,
            reboot_recovery,
            power_off_device,
            unlock_bootloader,
            lock_bootloader,
            wipe_data,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{
        cancel_force_fastboot_session, display_safety_class, filter_actions, load_flash_plan,
        normalize_storage_label, parse_flash_mode, parse_plan_request,
        plan_requires_connected_device, plan_to_dto, resolve_image_path_for_action,
        start_force_fastboot_session, store_flash_plan, update_overall_progress, AppState,
        FlashRunControl, ForceFastbootState, StoredPlans,
    };
    use fastboot_flasher::plan::slot_to_scatter;
    use mtk_scatter_parser::{FlashAction, FlashPlan, FlashPlanSummary};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
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
    fn live_flash_plan_requires_a_connected_device() {
        let mut plan = flash_plan(vec![flash_action("boot", "flash")]);
        plan.mode = "clean_flash".to_string();

        assert!(plan_requires_connected_device(&plan));
    }

    #[test]
    fn stored_plan_ids_resolve_exact_plans() {
        let state = AppState {
            device: Mutex::new(None),
            flash_plans: Mutex::new(StoredPlans {
                next_id: 1,
                plans: HashMap::new(),
            }),
            flash_control: FlashRunControl::default(),
            force_fastboot: Mutex::new(ForceFastbootState::default()),
        };

        let dry_run_id = store_flash_plan(&state, flash_plan(vec![flash_action("boot", "flash")]));
        let mut clean_flash = flash_plan(vec![flash_action("boot", "flash")]);
        clean_flash.mode = "clean_flash".to_string();
        let clean_flash_id = store_flash_plan(&state, clean_flash);

        assert_eq!(load_flash_plan(&state, dry_run_id).unwrap().mode, "dry-run");
        assert_eq!(
            load_flash_plan(&state, clean_flash_id).unwrap().mode,
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
    fn unknown_safety_class_displays_as_other() {
        assert_eq!(display_safety_class("unknown"), "other");
        assert_eq!(display_safety_class("boot_critical"), "boot_critical");
    }

    #[test]
    fn force_fastboot_sessions_are_replaced_and_cancellable() {
        let state = AppState {
            device: Mutex::new(None),
            flash_plans: Mutex::new(StoredPlans {
                next_id: 1,
                plans: HashMap::new(),
            }),
            flash_control: FlashRunControl::default(),
            force_fastboot: Mutex::new(ForceFastbootState::default()),
        };

        let first = start_force_fastboot_session(&state);
        let second = start_force_fastboot_session(&state);

        assert_eq!(first, 1);
        assert_eq!(second, 2);
        assert!(!cancel_force_fastboot_session(&state, first));
        assert!(cancel_force_fastboot_session(&state, second));
        assert!(!cancel_force_fastboot_session(&state, second));
    }
}
