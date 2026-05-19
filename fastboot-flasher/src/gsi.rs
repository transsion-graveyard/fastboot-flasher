use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use fastboot_rs::{prepare_image, PreparedImage};
use tempfile::TempDir;
use tokio::time::{sleep, timeout, Duration as TokioDuration};

use crate::{
    connect_fastboot, flash_one_partition,
    format::{
        detect_userdata, erase_optional_partition, generate_userdata_image, parse_fastboot_u64,
        FormatTools, FormatUserdataOptions, OptionalEraseOutcome, UserdataInfo, WipeDataOptions,
    },
    manual::resolved_disable_vbmeta_image_path,
    read_all_variables, read_variable, reboot_device_bootloader, reboot_device_fastboot,
    resolve_max_download_size_from_vars,
    FastbootDevice,
};

pub const PRODUCT_GSI_SIZE_BYTES: u64 = 335_872;
pub const PRODUCT_GSI_BLOCK_SIZE: u64 = 4_096;
pub const PRODUCT_GSI_BLOCKS: u64 = 82;
pub const PRODUCT_GSI_LABEL: &str = "product";
pub const PRODUCT_GSI_UUID: &str = "cdd462dd-8dd0-4006-8a5a-94e5a70c2bc3";
const MODE_WAIT_ATTEMPTS: usize = 20;
const MODE_WAIT_DELAY_MS: u64 = 250;
const MODE_TRANSITION_ATTEMPTS: usize = 2;

fn check_cancelled(token: &Option<Arc<AtomicBool>>) -> anyhow::Result<()> {
    if let Some(ref flag) = token {
        if flag.load(Ordering::Relaxed) {
            anyhow::bail!("cancelled by user");
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastbootMode {
    Bootloader,
    Fastbootd,
}

impl FastbootMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bootloader => "bootloader",
            Self::Fastbootd => "fastbootd",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GsiStep {
    RebootingToBootloader,
    RebootingToFastbootd,
    PreparingVbmetaFlash,
    FlashingVbmeta,
    CheckingSystemPartition,
    CheckingProductGsiFallback,
    GeneratingProductGsiImage,
    FlashingProductGsi,
    ProductGsiFallbackNotNeeded,
    FlashingSystemGsi,
    WipingUserdata,
    StartingBootloaderPhase,
    StartingFastbootdPhase,
    GsiFlowComplete,
}

impl GsiStep {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RebootingToBootloader => "rebooting to bootloader",
            Self::RebootingToFastbootd => "rebooting to fastbootd",
            Self::PreparingVbmetaFlash => "preparing vbmeta flash",
            Self::FlashingVbmeta => "flashing vbmeta",
            Self::CheckingSystemPartition => "checking system partition",
            Self::CheckingProductGsiFallback => "checking product_gsi fallback",
            Self::GeneratingProductGsiImage => "generating product_gsi image",
            Self::FlashingProductGsi => "flashing product_gsi",
            Self::ProductGsiFallbackNotNeeded => "product_gsi fallback not needed",
            Self::FlashingSystemGsi => "flashing system GSI",
            Self::WipingUserdata => "wiping userdata",
            Self::StartingBootloaderPhase => "starting bootloader phase",
            Self::StartingFastbootdPhase => "starting fastbootd phase",
            Self::GsiFlowComplete => "gsi flow complete",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GsiEvent {
    Step(GsiStep),
    ModeDetected(FastbootMode),
    ModeReady(FastbootMode),
    UserdataEraseFallback {
        fs_type: String,
    },
    ResolvedPartition {
        base: &'static str,
        partition: String,
        size_bytes: u64,
    },
    Flashing {
        partition: String,
        image: PathBuf,
        size_bytes: u64,
    },
    FlashProgress {
        partition: String,
        bytes: u64,
        total_bytes: u64,
        speed_bps: u64,
    },
    FlashFinished {
        partition: String,
        size_bytes: u64,
    },
    Erasing {
        partition: &'static str,
    },
    EraseFinished {
        partition: &'static str,
    },
    PartitionSkipped {
        partition: &'static str,
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct GsiFlashOptions {
    pub wipe_data: WipeDataOptions,
    pub cancel_token: Option<Arc<AtomicBool>>,
}

impl Default for GsiFlashOptions {
    fn default() -> Self {
        Self {
            wipe_data: WipeDataOptions::default(),
            cancel_token: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GsiFlashSummary {
    pub flash_count: usize,
    pub wipe_count: usize,
    pub skipped_count: usize,
    pub total_bytes: u64,
}

pub struct GsiFlashOutcome {
    pub device: FastbootDevice,
    pub summary: GsiFlashSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FastbootCapabilities {
    max_download_size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GsiExecutionPlan {
    pub summary: GsiFlashSummary,
    pub start_mode: FastbootMode,
    pub needs_product_gsi: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedProductGsiSpec {
    pub size_bytes: u64,
    pub block_size: u64,
    pub blocks: u64,
    pub label: &'static str,
    pub uuid: &'static str,
}

#[derive(Debug)]
pub struct GeneratedProductGsiImage {
    temp_dir: TempDir,
    path: PathBuf,
}

impl GeneratedProductGsiImage {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn keepalive_dir(&self) -> &Path {
        self.temp_dir.path()
    }
}

pub fn fixed_product_gsi_spec() -> FixedProductGsiSpec {
    FixedProductGsiSpec {
        size_bytes: PRODUCT_GSI_SIZE_BYTES,
        block_size: PRODUCT_GSI_BLOCK_SIZE,
        blocks: PRODUCT_GSI_BLOCKS,
        label: PRODUCT_GSI_LABEL,
        uuid: PRODUCT_GSI_UUID,
    }
}

pub fn resolve_target_partition(
    base: &str,
    current_slot: &str,
    available: &HashSet<String>,
) -> anyhow::Result<String> {
    let active = format!("{base}_{current_slot}");
    if available.contains(&active) {
        return Ok(active);
    }
    if available.contains(base) {
        return Ok(base.to_string());
    }

    anyhow::bail!("missing fastboot partition for `{base}` on slot `{current_slot}`")
}

pub fn should_flash_product_gsi(system_partition_size: u64, gsi_expanded_size: u64) -> bool {
    gsi_expanded_size > system_partition_size
}

pub fn inspect_gsi_image(image: &Path) -> anyhow::Result<PreparedImage> {
    prepare_image(image, u32::MAX).with_context(|| format!("inspect GSI image {}", image.display()))
}

pub fn build_gsi_execution_plan(
    start_mode: FastbootMode,
    image_size: u64,
    vbmeta_size: u64,
    _userdata: &UserdataInfo,
    options: &GsiFlashOptions,
    needs_product_gsi: Option<bool>,
) -> GsiExecutionPlan {
    // The execution plan is built before userdata.img is generated.
    // Using the full partition size here inflates UI totals for sparse/empty images,
    // especially on large userdata partitions.
    let userdata_bytes = 1;

    let mut summary = GsiFlashSummary {
        flash_count: 2,
        wipe_count: 1,
        skipped_count: 0,
        total_bytes: image_size
            .saturating_add(vbmeta_size)
            .saturating_add(userdata_bytes),
    };

    if options.wipe_data.erase_metadata {
        summary.wipe_count += 1;
        summary.total_bytes = summary.total_bytes.saturating_add(1);
    }
    if options.wipe_data.erase_cache {
        summary.wipe_count += 1;
        summary.total_bytes = summary.total_bytes.saturating_add(1);
    }
    if needs_product_gsi == Some(true) {
        summary.flash_count += 1;
        summary.total_bytes = summary.total_bytes.saturating_add(PRODUCT_GSI_SIZE_BYTES);
    }

    GsiExecutionPlan {
        summary,
        start_mode,
        needs_product_gsi,
    }
}

pub fn generate_product_gsi_image(tools: &FormatTools) -> anyhow::Result<GeneratedProductGsiImage> {
    tools.validate()?;

    let spec = fixed_product_gsi_spec();
    let temp_dir = tempfile::Builder::new()
        .prefix("fastboot-flasher-gsi-")
        .tempdir()
        .context("create temp directory for product_gsi image")?;
    let path = temp_dir.path().join("product_gsi.img");

    std::fs::File::create(&path)
        .with_context(|| format!("create {}", path.display()))?
        .set_len(spec.size_bytes)
        .with_context(|| format!("set size for {}", path.display()))?;

    let mut cmd = Command::new(&tools.mke2fs);
    cmd.arg("-F")
        .arg("-t")
        .arg("ext4")
        .arg("-b")
        .arg(spec.block_size.to_string())
        .arg("-L")
        .arg(spec.label)
        .arg("-U")
        .arg(spec.uuid)
        .arg(&path)
        .arg(spec.blocks.to_string())
        .env("MKE2FS_CONFIG", &tools.mke2fs_conf);
    tools.apply_runtime_env(&mut cmd)?;

    let output = cmd.output().context("run mke2fs for product_gsi")?;
    anyhow::ensure!(
        output.status.success(),
        "mke2fs failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );

    Ok(GeneratedProductGsiImage { temp_dir, path })
}

pub fn detect_fastboot_mode(vars: &HashMap<String, String>) -> FastbootMode {
    match vars.get("is-userspace").map(String::as_str) {
        Some("yes") => FastbootMode::Fastbootd,
        _ => FastbootMode::Bootloader,
    }
}

fn normalize_slot(slot: Option<&String>) -> Option<String> {
    match slot.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if value == "a" || value == "b" => Some(value),
        _ => None,
    }
}

fn available_partitions(vars: &HashMap<String, String>) -> HashSet<String> {
    vars.keys()
        .filter_map(|key| key.strip_prefix("partition-size:"))
        .map(ToOwned::to_owned)
        .collect()
}

fn partition_var_name(partition: &str) -> String {
    format!("partition-size:{partition}")
}

async fn try_partition_size(
    dev: &mut FastbootDevice,
    vars: &mut HashMap<String, String>,
    partition: &str,
) -> anyhow::Result<Option<u64>> {
    let key = partition_var_name(partition);
    if let Some(value) = vars.get(&key) {
        return parse_fastboot_u64(value)
            .map(Some)
            .with_context(|| format!("parse {key}"));
    }

    match read_variable(dev, &key).await {
        Ok(value) => {
            let size = parse_fastboot_u64(&value).with_context(|| format!("parse {key}"))?;
            vars.insert(key, value);
            Ok(Some(size))
        }
        Err(_) => Ok(None),
    }
}

async fn resolve_device_partition(
    dev: &mut FastbootDevice,
    vars: &mut HashMap<String, String>,
    base: &'static str,
    current_slot: Option<&str>,
) -> anyhow::Result<(String, u64)> {
    if let Some(slot) = current_slot {
        let active = format!("{base}_{slot}");
        if let Some(size) = try_partition_size(dev, vars, &active).await? {
            return Ok((active, size));
        }
    }

    if let Some(size) = try_partition_size(dev, vars, base).await? {
        return Ok((base.to_string(), size));
    }

    let available = available_partitions(vars);
    let slot = current_slot.unwrap_or("unknown");
    resolve_target_partition(base, slot, &available)
        .map(|partition| (partition, 0))
}

async fn wait_for_device_vars() -> anyhow::Result<(FastbootDevice, HashMap<String, String>)> {
    let mut last_error = None;

    for _ in 0..MODE_WAIT_ATTEMPTS {
        let mut dev = match connect_fastboot().await {
            Ok(dev) => dev,
            Err(error) => {
                last_error = Some(error);
                sleep(Duration::from_millis(MODE_WAIT_DELAY_MS)).await;
                continue;
            }
        };
        match read_all_variables(&mut dev).await {
            Ok(vars) => return Ok((dev, vars)),
            Err(error) => {
                last_error = Some(error);
                sleep(Duration::from_millis(MODE_WAIT_DELAY_MS)).await;
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("timed out waiting for fastboot variables")))
}

fn resolve_fastboot_capabilities(vars: &HashMap<String, String>) -> anyhow::Result<FastbootCapabilities> {
    let max_download_size = resolve_max_download_size_from_vars(vars)?;
    eprintln!("[gsi-shared] resolved max-download-size=0x{max_download_size:x}");
    Ok(FastbootCapabilities { max_download_size })
}

pub async fn maybe_needs_product_gsi(
    dev: &mut FastbootDevice,
    vars: &HashMap<String, String>,
    gsi_expanded_size: u64,
) -> anyhow::Result<Option<bool>> {
    let mut vars = vars.clone();
    let current_slot = normalize_slot(vars.get("current-slot"));
    match resolve_device_partition(dev, &mut vars, "system", current_slot.as_deref()).await {
        Ok((_, system_partition_size)) => Ok(Some(should_flash_product_gsi(
            system_partition_size,
            gsi_expanded_size,
        ))),
        Err(_) => Ok(None),
    }
}

const MODE_TRANSITION_TIMEOUT_SECS: u64 = 120;

async fn transition_mode(
    mut dev: FastbootDevice,
    vars: HashMap<String, String>,
    target_mode: FastbootMode,
    report: &mut impl FnMut(GsiEvent),
) -> anyhow::Result<(FastbootDevice, HashMap<String, String>, FastbootCapabilities)> {
    if detect_fastboot_mode(&vars) == target_mode {
        report(GsiEvent::ModeReady(target_mode));
        let capabilities = resolve_fastboot_capabilities(&vars)?;
        return Ok((dev, vars, capabilities));
    }

    let fut = async {
        for _ in 0..MODE_TRANSITION_ATTEMPTS {
        match target_mode {
            FastbootMode::Bootloader => {
                report(GsiEvent::Step(GsiStep::RebootingToBootloader));
                reboot_device_bootloader(&mut dev).await?;
            }
            FastbootMode::Fastbootd => {
                report(GsiEvent::Step(GsiStep::RebootingToFastbootd));
                reboot_device_fastboot(&mut dev).await?;
            }
        }
        drop(dev);

        let (next_dev, next_vars) = wait_for_device_vars().await?;
        let next_mode = detect_fastboot_mode(&next_vars);
        if next_mode == target_mode {
            report(GsiEvent::ModeReady(target_mode));
            let capabilities = resolve_fastboot_capabilities(&next_vars)?;
            return Ok((next_dev, next_vars, capabilities));
        }
        dev = next_dev;
    }

        anyhow::bail!(
            "GSI flow required {}, but the device did not switch modes after retry",
            target_mode.as_str()
        )
    };

    timeout(TokioDuration::from_secs(MODE_TRANSITION_TIMEOUT_SECS), fut)
        .await
        .map_err(|_| anyhow::anyhow!("mode transition timed out after {MODE_TRANSITION_TIMEOUT_SECS}s"))?
}

async fn flash_vbmeta_logged(
    dev: &mut FastbootDevice,
    vars: &mut HashMap<String, String>,
    capabilities: &FastbootCapabilities,
    image: &Path,
    size_bytes: u64,
    summary: &mut GsiFlashSummary,
    report: &mut impl FnMut(GsiEvent),
    options: &GsiFlashOptions,
) -> anyhow::Result<()> {
    check_cancelled(&options.cancel_token)?;
    report(GsiEvent::Step(GsiStep::PreparingVbmetaFlash));
    let current_slot = normalize_slot(vars.get("current-slot"));
    let (vbmeta_partition, vbmeta_partition_size) =
        resolve_device_partition(dev, vars, "vbmeta", current_slot.as_deref()).await?;
    report(GsiEvent::ResolvedPartition {
        base: "vbmeta",
        partition: vbmeta_partition.clone(),
        size_bytes: vbmeta_partition_size,
    });
    report(GsiEvent::Step(GsiStep::FlashingVbmeta));
    flash_partition_logged(
        dev,
        &vbmeta_partition,
        image,
        size_bytes,
        capabilities.max_download_size,
        summary,
        report,
        options,
    )
    .await
}

async fn flash_fastbootd_gsi_logged(
    dev: &mut FastbootDevice,
    vars: &mut HashMap<String, String>,
    capabilities: &FastbootCapabilities,
    image: &Path,
    image_size: u64,
    gsi_expanded_size: u64,
    tools: &FormatTools,
    summary: &mut GsiFlashSummary,
    report: &mut impl FnMut(GsiEvent),
    options: &GsiFlashOptions,
) -> anyhow::Result<()> {
    check_cancelled(&options.cancel_token)?;
    report(GsiEvent::Step(GsiStep::CheckingSystemPartition));
    let current_slot = normalize_slot(vars.get("current-slot"));
    let (system_partition, system_partition_size) =
        resolve_device_partition(dev, vars, "system", current_slot.as_deref()).await?;
    report(GsiEvent::ResolvedPartition {
        base: "system",
        partition: system_partition.clone(),
        size_bytes: system_partition_size,
    });

    report(GsiEvent::Step(GsiStep::CheckingProductGsiFallback));
    if should_flash_product_gsi(system_partition_size, gsi_expanded_size) {
        check_cancelled(&options.cancel_token)?;
        let (product_partition, product_partition_size) =
            resolve_device_partition(dev, vars, "product", current_slot.as_deref()).await?;
        report(GsiEvent::ResolvedPartition {
            base: "product",
            partition: product_partition.clone(),
            size_bytes: product_partition_size,
        });
        report(GsiEvent::Step(GsiStep::GeneratingProductGsiImage));
        let product_image = generate_product_gsi_image(tools)?;
        let product_size = std::fs::metadata(product_image.path())
            .with_context(|| format!("read image metadata for {}", product_image.path().display()))?
            .len();
        report(GsiEvent::Step(GsiStep::FlashingProductGsi));
        eprintln!("[gsi-shared] after step flashing_product_gsi");
        eprintln!("[gsi-shared] before create flash_partition_logged future partition={} image={} size_bytes={}", product_partition, product_image.path().display(), product_size);
        let flash_future = flash_partition_logged(
            dev,
            &product_partition,
            product_image.path(),
            product_size,
            capabilities.max_download_size,
            summary,
            report,
            options,
        );
        eprintln!("[gsi-shared] after create flash_partition_logged future partition={} size_bytes={}", product_partition, product_size);
        flash_future.await?;
    } else {
        report(GsiEvent::Step(GsiStep::ProductGsiFallbackNotNeeded));
    }

    check_cancelled(&options.cancel_token)?;
    report(GsiEvent::Step(GsiStep::FlashingSystemGsi));
    flash_partition_logged(
        dev,
        &system_partition,
        image,
        image_size,
        capabilities.max_download_size,
        summary,
        report,
        options,
    )
    .await
}

async fn flash_partition_logged(
    dev: &mut FastbootDevice,
    partition: &str,
    image: &Path,
    size_bytes: u64,
    max_download_size: u32,
    summary: &mut GsiFlashSummary,
    report: &mut impl FnMut(GsiEvent),
    options: &GsiFlashOptions,
) -> anyhow::Result<()> {
    check_cancelled(&options.cancel_token)?;
    eprintln!(
        "[gsi-shared] before report Flushing-like event partition={} image={} size_bytes={}",
        partition,
        image.display(),
        size_bytes
    );
    report(GsiEvent::Flashing {
        partition: partition.to_string(),
        image: image.to_path_buf(),
        size_bytes,
    });
    eprintln!(
        "[gsi-shared] after report Flashing partition={} size_bytes={}",
        partition,
        size_bytes
    );
    let partition_name = partition.to_string();
    let mut bytes_flashed = 0_u64;
    let started = std::time::Instant::now();
    flash_one_partition(dev, partition, image, max_download_size, |event| {
        if let crate::FlashProgress::DownloadBytes { bytes, .. } = event {
            bytes_flashed += bytes;
            let speed_bps = match started.elapsed().as_secs_f64() {
                secs if secs > 0.0 => (bytes_flashed as f64 / secs) as u64,
                _ => 0,
            };
            report(GsiEvent::FlashProgress {
                partition: partition_name.clone(),
                bytes: bytes_flashed,
                total_bytes: size_bytes.max(1),
                speed_bps,
            });
        }
    })
    .await?;
    check_cancelled(&options.cancel_token)?;
    report(GsiEvent::FlashFinished {
        partition: partition.to_string(),
        size_bytes,
    });
    summary.flash_count += 1;
    summary.total_bytes = summary.total_bytes.saturating_add(size_bytes);
    Ok(())
}

async fn erase_optional_partition_logged(
    dev: &mut FastbootDevice,
    partition: &'static str,
    summary: &mut GsiFlashSummary,
    report: &mut impl FnMut(GsiEvent),
    options: &GsiFlashOptions,
) -> anyhow::Result<()> {
    check_cancelled(&options.cancel_token)?;
    report(GsiEvent::Erasing { partition });
    match erase_optional_partition(dev, partition).await? {
        OptionalEraseOutcome::Erased => {
            summary.wipe_count += 1;
            report(GsiEvent::EraseFinished { partition });
        }
        OptionalEraseOutcome::Skipped { reason } => {
            summary.skipped_count += 1;
            report(GsiEvent::PartitionSkipped { partition, reason });
        }
    }
    summary.total_bytes = summary.total_bytes.saturating_add(1);
    Ok(())
}

async fn wipe_userdata_logged(
    dev: &mut FastbootDevice,
    tools: &FormatTools,
    info: &crate::format::UserdataInfo,
    wipe_data: &WipeDataOptions,
    summary: &mut GsiFlashSummary,
    report: &mut impl FnMut(GsiEvent),
    options: &GsiFlashOptions,
) -> anyhow::Result<()> {
    check_cancelled(&options.cancel_token)?;
    report(GsiEvent::Step(GsiStep::WipingUserdata));
    match generate_userdata_image(
        tools,
        info,
        &FormatUserdataOptions {
            erase_fallback: wipe_data.erase_fallback,
            casefold: wipe_data.casefold,
        },
    ) {
        Ok(generated) => {
            let userdata_bytes = generated.image_len()?;
            flash_partition_logged(
                dev,
                "userdata",
                generated.path(),
                userdata_bytes,
                info.max_download_size
                    .and_then(|value| u32::try_from(value).ok())
                    .context("missing userdata max-download-size")?,
                summary,
                report,
                options,
            )
            .await?;
            summary.wipe_count += 1;
        }
        Err(_error)
            if wipe_data.erase_fallback
                || info.fs_type.eq_ignore_ascii_case("raw") =>
        {
            check_cancelled(&options.cancel_token)?;
            report(GsiEvent::UserdataEraseFallback {
                fs_type: info.fs_type.clone(),
            });
            report(GsiEvent::Erasing {
                partition: "userdata",
            });
            dev.erase("userdata")
                .await
                .map_err(anyhow::Error::from)
                .context("erase userdata fallback")?;
            summary.wipe_count += 1;
            summary.total_bytes = summary.total_bytes.saturating_add(1);
            report(GsiEvent::EraseFinished {
                partition: "userdata",
            });
        }
        Err(error) => return Err(error).context("generate userdata image"),
    }

    if wipe_data.erase_metadata {
        erase_optional_partition_logged(dev, "metadata", summary, report, options).await?;
    }
    if wipe_data.erase_cache {
        erase_optional_partition_logged(dev, "cache", summary, report, options).await?;
    }

    Ok(())
}

pub async fn execute_gsi_flash(
    mut dev: FastbootDevice,
    image: &Path,
    tools: &FormatTools,
    options: &GsiFlashOptions,
    report: impl FnMut(GsiEvent),
) -> anyhow::Result<GsiFlashOutcome> {
    let vars = read_all_variables(&mut dev).await?;
    execute_gsi_flash_with_vars(dev, vars, image, tools, options, report).await
}

pub async fn execute_gsi_flash_with_vars(
    mut dev: FastbootDevice,
    mut vars: HashMap<String, String>,
    image: &Path,
    tools: &FormatTools,
    options: &GsiFlashOptions,
    mut report: impl FnMut(GsiEvent),
) -> anyhow::Result<GsiFlashOutcome> {
    let metadata = std::fs::metadata(image)
        .with_context(|| format!("read GSI image metadata for {}", image.display()))?;
    anyhow::ensure!(metadata.is_file(), "{} is not a regular file", image.display());

    let inspected =
        inspect_gsi_image(image).with_context(|| format!("inspect GSI image {}", image.display()))?;
    let start_mode = detect_fastboot_mode(&vars);
    let mut capabilities = resolve_fastboot_capabilities(&vars)?;
    report(GsiEvent::ModeDetected(start_mode));

    let userdata = detect_userdata(&mut dev).await?;
    let vbmeta_image = resolved_disable_vbmeta_image_path()?;
    let vbmeta_size = std::fs::metadata(&vbmeta_image)
        .with_context(|| format!("read image metadata for {}", vbmeta_image.display()))?
        .len();

    let mut summary = GsiFlashSummary::default();

    match start_mode {
        FastbootMode::Bootloader => {
            report(GsiEvent::Step(GsiStep::StartingBootloaderPhase));
            check_cancelled(&options.cancel_token)?;
            flash_vbmeta_logged(
                &mut dev,
                &mut vars,
                &capabilities,
                &vbmeta_image,
                vbmeta_size,
                &mut summary,
                &mut report,
                options,
            )
            .await?;
            check_cancelled(&options.cancel_token)?;
            wipe_userdata_logged(
                &mut dev,
                tools,
                &userdata,
                &options.wipe_data,
                &mut summary,
                &mut report,
                options,
            )
            .await?;

            check_cancelled(&options.cancel_token)?;
            let transitioned =
                transition_mode(dev, vars, FastbootMode::Fastbootd, &mut report).await?;
            dev = transitioned.0;
            vars = transitioned.1;
            capabilities = transitioned.2;

            check_cancelled(&options.cancel_token)?;
            flash_fastbootd_gsi_logged(
                &mut dev,
                &mut vars,
                &capabilities,
                image,
                metadata.len(),
                inspected.expanded_size,
                tools,
                &mut summary,
                &mut report,
                options,
            )
            .await?;
        }
        FastbootMode::Fastbootd => {
            report(GsiEvent::Step(GsiStep::StartingFastbootdPhase));
            check_cancelled(&options.cancel_token)?;
            flash_fastbootd_gsi_logged(
                &mut dev,
                &mut vars,
                &capabilities,
                image,
                metadata.len(),
                inspected.expanded_size,
                tools,
                &mut summary,
                &mut report,
                options,
            )
            .await?;

            check_cancelled(&options.cancel_token)?;
            let transitioned =
                transition_mode(dev, vars, FastbootMode::Bootloader, &mut report).await?;
            dev = transitioned.0;
            vars = transitioned.1;
            capabilities = transitioned.2;

            check_cancelled(&options.cancel_token)?;
            flash_vbmeta_logged(
                &mut dev,
                &mut vars,
                &capabilities,
                &vbmeta_image,
                vbmeta_size,
                &mut summary,
                &mut report,
                options,
            )
            .await?;
            check_cancelled(&options.cancel_token)?;
            wipe_userdata_logged(
                &mut dev,
                tools,
                &userdata,
                &options.wipe_data,
                &mut summary,
                &mut report,
                options,
            )
            .await?;

            check_cancelled(&options.cancel_token)?;
            let transitioned =
                transition_mode(dev, vars, FastbootMode::Fastbootd, &mut report).await?;
            dev = transitioned.0;
            let _ = transitioned.1;
            let _ = transitioned.2;
        }
    }

    report(GsiEvent::Step(GsiStep::GsiFlowComplete));
    report(GsiEvent::ModeReady(FastbootMode::Fastbootd));
    Ok(GsiFlashOutcome { device: dev, summary })
}

#[cfg(test)]
mod tests {
    use super::{
        build_gsi_execution_plan, detect_fastboot_mode, fixed_product_gsi_spec, normalize_slot,
        resolve_target_partition, should_flash_product_gsi, FastbootMode, GsiFlashOptions,
        MODE_TRANSITION_TIMEOUT_SECS, PRODUCT_GSI_BLOCKS, PRODUCT_GSI_BLOCK_SIZE,
        PRODUCT_GSI_LABEL, PRODUCT_GSI_SIZE_BYTES, PRODUCT_GSI_UUID,
    };
    use crate::format::{UserdataInfo, WipeDataOptions};
    use std::collections::{HashMap, HashSet};

    #[test]
    fn fixed_product_gsi_spec_matches_expected_recipe() {
        let spec = fixed_product_gsi_spec();

        assert_eq!(spec.size_bytes, PRODUCT_GSI_SIZE_BYTES);
        assert_eq!(spec.block_size, PRODUCT_GSI_BLOCK_SIZE);
        assert_eq!(spec.blocks, PRODUCT_GSI_BLOCKS);
        assert_eq!(spec.label, PRODUCT_GSI_LABEL);
        assert_eq!(spec.uuid, PRODUCT_GSI_UUID);
    }

    #[test]
    fn resolve_target_partition_prefers_active_slot_partition() {
        let available =
            HashSet::from(["system_a".to_string(), "system_b".to_string(), "system".to_string()]);

        let resolved = resolve_target_partition("system", "a", &available).unwrap();

        assert_eq!(resolved, "system_a");
    }

    #[test]
    fn resolve_target_partition_falls_back_to_unsuffixed_partition() {
        let available = HashSet::from(["vbmeta".to_string()]);

        let resolved = resolve_target_partition("vbmeta", "b", &available).unwrap();

        assert_eq!(resolved, "vbmeta");
    }

    #[test]
    fn should_flash_product_gsi_when_gsi_exceeds_system_partition_size() {
        assert!(should_flash_product_gsi(128, 256));
        assert!(!should_flash_product_gsi(256, 128));
    }

    #[test]
    fn detect_fastboot_mode_treats_is_userspace_yes_as_fastbootd() {
        let vars = HashMap::from([("is-userspace".to_string(), "yes".to_string())]);

        assert_eq!(detect_fastboot_mode(&vars), FastbootMode::Fastbootd);
    }

    #[test]
    fn detect_fastboot_mode_treats_missing_or_non_yes_as_bootloader() {
        let missing = HashMap::new();
        let no = HashMap::from([("is-userspace".to_string(), "no".to_string())]);

        assert_eq!(detect_fastboot_mode(&missing), FastbootMode::Bootloader);
        assert_eq!(detect_fastboot_mode(&no), FastbootMode::Bootloader);
    }

    #[test]
    fn normalize_slot_only_accepts_a_and_b() {
        assert_eq!(normalize_slot(Some(&"a".to_string())).as_deref(), Some("a"));
        assert_eq!(normalize_slot(Some(&"B".to_string())).as_deref(), Some("b"));
        assert_eq!(normalize_slot(Some(&"other".to_string())), None);
    }

    #[test]
    fn build_gsi_execution_plan_counts_known_flashes_and_wipes() {
        let plan = build_gsi_execution_plan(
            FastbootMode::Bootloader,
            1_000,
            4,
            &UserdataInfo {
                fs_type: "raw".to_string(),
                size: 8_192,
                max_download_size: None,
                erase_block_size: None,
                logical_block_size: None,
            },
            &GsiFlashOptions {
                wipe_data: WipeDataOptions::default(),
                cancel_token: None,
            },
            Some(true),
        );

        assert_eq!(plan.start_mode, FastbootMode::Bootloader);
        assert_eq!(plan.summary.flash_count, 3);
        assert_eq!(plan.summary.wipe_count, 3);
        assert_eq!(plan.summary.skipped_count, 0);
        assert_eq!(
            plan.summary.total_bytes,
            1_000 + 4 + 1 + PRODUCT_GSI_SIZE_BYTES + 1 + 1
        );
    }

    #[test]
    fn build_gsi_execution_plan_does_not_use_full_userdata_partition_size_for_non_raw_wipe() {
        let plan = build_gsi_execution_plan(
            FastbootMode::Fastbootd,
            2_048,
            8,
            &UserdataInfo {
                fs_type: "ext4".to_string(),
                size: 256 * 1024 * 1024 * 1024,
                max_download_size: None,
                erase_block_size: None,
                logical_block_size: None,
            },
            &GsiFlashOptions {
                wipe_data: WipeDataOptions::default(),
                cancel_token: None,
            },
            Some(false),
        );

        assert_eq!(plan.summary.flash_count, 2);
        assert_eq!(plan.summary.wipe_count, 3);
        assert_eq!(plan.summary.skipped_count, 0);
        assert_eq!(plan.summary.total_bytes, 2_048 + 8 + 1 + 1 + 1);
    }

    #[test]
    fn mode_transition_timeout_is_two_minutes() {
        assert_eq!(MODE_TRANSITION_TIMEOUT_SECS, 120);
    }
}
