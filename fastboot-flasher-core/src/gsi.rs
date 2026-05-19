//! GSI (Generic System Image) flashing flow: detect mode, transition between
//! bootloader and fastbootd, flash vbmeta + system + optional product_gsi, and
//! wipe userdata.

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
use tracing::debug;

use crate::{
    connect_fastboot, flash_one_partition,
    format::{
        detect_userdata, erase_optional_partition, generate_userdata_image, parse_fastboot_u64,
        FormatTools, FormatUserdataOptions, OptionalEraseOutcome, UserdataInfo, WipeDataOptions,
    },
    manual::resolved_disable_vbmeta_image_path,
    read_all_variables, read_variable, reboot_device_bootloader, reboot_device_fastboot,
    resolve_max_download_size_from_vars, FastbootDevice,
};

/// Size in bytes of the product_gsi fallback image.
pub const PRODUCT_GSI_SIZE_BYTES: u64 = 335_872;
/// Block size for the product_gsi image.
pub const PRODUCT_GSI_BLOCK_SIZE: u64 = 4_096;
/// Number of blocks in the product_gsi image.
pub const PRODUCT_GSI_BLOCKS: u64 = 82;
/// Filesystem label for the product_gsi image.
pub const PRODUCT_GSI_LABEL: &str = "product";
/// UUID for the product_gsi ext4 filesystem.
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

/// The fastboot mode the device is currently in (or should transition to).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastbootMode {
    /// Traditional bootloader fastboot.
    Bootloader,
    /// Userspace fastboot (fastbootd).
    Fastbootd,
}

impl FastbootMode {
    /// Return the string representation of the mode.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bootloader => "bootloader",
            Self::Fastbootd => "fastbootd",
        }
    }
}

/// A step in the GSI flashing sequence, used for progress reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GsiStep {
    /// Rebooting into bootloader mode.
    RebootingToBootloader,
    /// Rebooting into fastbootd mode.
    RebootingToFastbootd,
    /// Preparing to flash the vbmeta image.
    PreparingVbmetaFlash,
    /// Flashing the vbmeta image.
    FlashingVbmeta,
    /// Checking the system partition size.
    CheckingSystemPartition,
    /// Checking whether product_gsi fallback is needed.
    CheckingProductGsiFallback,
    /// Generating a product_gsi ext4 image.
    GeneratingProductGsiImage,
    /// Flashing the product_gsi image.
    FlashingProductGsi,
    /// product_gsi fallback was not required.
    ProductGsiFallbackNotNeeded,
    /// Flashing the main GSI system image.
    FlashingSystemGsi,
    /// Wiping the userdata partition.
    WipingUserdata,
    /// Starting the bootloader phase of the flow.
    StartingBootloaderPhase,
    /// Starting the fastbootd phase of the flow.
    StartingFastbootdPhase,
    /// The entire GSI flow is complete.
    GsiFlowComplete,
}

impl GsiStep {
    /// Return a human-readable label for this step.
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

/// Events emitted during the GSI flash flow for progress/UI reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GsiEvent {
    /// A named step in the GSI sequence.
    Step(GsiStep),
    /// The device's current mode was detected.
    ModeDetected(FastbootMode),
    /// The device has confirmed it is in the requested mode.
    ModeReady(FastbootMode),
    /// Userdata erase fallback was used because image generation failed.
    UserdataEraseFallback {
        /// The filesystem type that could not be built.
        fs_type: String,
    },
    /// A partition was resolved to its actual device name.
    ResolvedPartition {
        /// Base partition name.
        base: &'static str,
        /// Resolved partition name (e.g. `system_a`).
        partition: String,
        /// Size of the partition in bytes.
        size_bytes: u64,
    },
    /// A flash operation has begun.
    Flashing {
        /// Target partition name.
        partition: String,
        /// Path to the image being flashed.
        image: PathBuf,
        /// Image size in bytes.
        size_bytes: u64,
    },
    /// Progress update during a flash.
    FlashProgress {
        /// Target partition name.
        partition: String,
        /// Bytes transferred so far.
        bytes: u64,
        /// Total bytes to transfer.
        total_bytes: u64,
        /// Transfer speed in bytes per second.
        speed_bps: u64,
    },
    /// A flash operation has completed.
    FlashFinished {
        /// Target partition name.
        partition: String,
        /// Image size in bytes.
        size_bytes: u64,
    },
    /// An erase operation has begun.
    Erasing {
        /// Partition being erased.
        partition: &'static str,
    },
    /// An erase operation has completed.
    EraseFinished {
        /// Partition that was erased.
        partition: &'static str,
    },
    /// A partition was skipped (e.g. it does not exist on the device).
    PartitionSkipped {
        /// Partition that was skipped.
        partition: &'static str,
        /// Reason for skipping.
        reason: String,
    },
}

/// Configuration options for a GSI flash operation.
#[derive(Debug, Clone, Default)]
pub struct GsiFlashOptions {
    /// Wipe-data options controlling userdata, metadata, and cache handling.
    pub wipe_data: WipeDataOptions,
    /// Optional cancellation token that aborts the flow when set to `true`.
    pub cancel_token: Option<Arc<AtomicBool>>,
}

/// Summary statistics for a completed GSI flash operation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GsiFlashSummary {
    /// Number of partitions flashed.
    pub flash_count: usize,
    /// Number of partitions wiped.
    pub wipe_count: usize,
    /// Number of skipped operations.
    pub skipped_count: usize,
    /// Total bytes transferred.
    pub total_bytes: u64,
}

/// The result of a GSI flash operation.
pub struct GsiFlashOutcome {
    /// The fastboot device after the operation.
    pub device: FastbootDevice,
    /// Summary of what was done.
    pub summary: GsiFlashSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FastbootCapabilities {
    max_download_size: u32,
}

/// Execution plan describing what will happen during a GSI flash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GsiExecutionPlan {
    /// Summary of planned operations.
    pub summary: GsiFlashSummary,
    /// The mode the device is expected to start in.
    pub start_mode: FastbootMode,
    /// Whether product_gsi fallback is needed (`None` = unknown).
    pub needs_product_gsi: Option<bool>,
}

/// Fixed specification for the product_gsi fallback image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedProductGsiSpec {
    /// Total image size in bytes.
    pub size_bytes: u64,
    /// Ext4 block size.
    pub block_size: u64,
    /// Number of blocks.
    pub blocks: u64,
    /// Filesystem label.
    pub label: &'static str,
    /// Filesystem UUID.
    pub uuid: &'static str,
}

/// A generated product_gsi ext4 image backed by a temporary directory.
#[derive(Debug)]
pub struct GeneratedProductGsiImage {
    temp_dir: TempDir,
    path: PathBuf,
}

impl GeneratedProductGsiImage {
    /// Path to the generated image file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Path to the temporary directory that keeps the image alive.
    pub fn keepalive_dir(&self) -> &Path {
        self.temp_dir.path()
    }
}

/// Return the fixed [`FixedProductGsiSpec`] for the bundled product_gsi image.
pub fn fixed_product_gsi_spec() -> FixedProductGsiSpec {
    FixedProductGsiSpec {
        size_bytes: PRODUCT_GSI_SIZE_BYTES,
        block_size: PRODUCT_GSI_BLOCK_SIZE,
        blocks: PRODUCT_GSI_BLOCKS,
        label: PRODUCT_GSI_LABEL,
        uuid: PRODUCT_GSI_UUID,
    }
}

/// Resolve the actual partition name for a given base name (e.g. "system")
/// given the current slot and the set of available partitions, preferring the
/// slot-suffixed variant.
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

/// Determine whether the product_gsi fallback image is needed because the GSI
/// expanded size exceeds the system partition size.
pub fn should_flash_product_gsi(system_partition_size: u64, gsi_expanded_size: u64) -> bool {
    gsi_expanded_size > system_partition_size
}

/// Prepare and inspect a GSI image to determine its expanded size.
pub fn inspect_gsi_image(image: &Path) -> anyhow::Result<PreparedImage> {
    prepare_image(image, u32::MAX).with_context(|| format!("inspect GSI image {}", image.display()))
}

/// Build a [`GsiExecutionPlan`] describing the estimated work for a GSI flash.
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

/// Generate a small ext4 product_gsi image using mke2fs.
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

/// Detect whether the device is in bootloader fastboot or userspace fastbootd
/// by inspecting the `is-userspace` variable.
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
    resolve_target_partition(base, slot, &available).map(|partition| (partition, 0))
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

fn resolve_fastboot_capabilities(
    vars: &HashMap<String, String>,
) -> anyhow::Result<FastbootCapabilities> {
    let max_download_size = resolve_max_download_size_from_vars(vars)?;
    debug!(
        max_download_size = %format!("0x{max_download_size:x}"),
        "resolved max-download-size"
    );
    Ok(FastbootCapabilities { max_download_size })
}

/// Check whether the product_gsi fallback may be needed by comparing system
/// partition size to the GSI expanded size.
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
) -> anyhow::Result<(
    FastbootDevice,
    HashMap<String, String>,
    FastbootCapabilities,
)> {
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
        .map_err(|_| {
            anyhow::anyhow!("mode transition timed out after {MODE_TRANSITION_TIMEOUT_SECS}s")
        })?
}

struct PartitionFlashContext<'a, F>
where
    F: FnMut(GsiEvent),
{
    dev: &'a mut FastbootDevice,
    summary: &'a mut GsiFlashSummary,
    report: &'a mut F,
    options: &'a GsiFlashOptions,
}

impl<'a, F> PartitionFlashContext<'a, F>
where
    F: FnMut(GsiEvent),
{
    async fn flash_partition_logged(
        &mut self,
        partition: &str,
        image: &Path,
        size_bytes: u64,
        max_download_size: u32,
    ) -> anyhow::Result<()> {
        check_cancelled(&self.options.cancel_token)?;
        debug!(
            partition,
            image = %image.display(),
            size_bytes,
            "before report flashing-like event"
        );
        (self.report)(GsiEvent::Flashing {
            partition: partition.to_string(),
            image: image.to_path_buf(),
            size_bytes,
        });
        debug!(partition, size_bytes, "after report Flashing");
        let partition_name = partition.to_string();
        let mut bytes_flashed = 0_u64;
        let started = std::time::Instant::now();
        let report = &mut *self.report;
        flash_one_partition(self.dev, partition, image, max_download_size, |event| {
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
        check_cancelled(&self.options.cancel_token)?;
        (self.report)(GsiEvent::FlashFinished {
            partition: partition.to_string(),
            size_bytes,
        });
        self.summary.flash_count += 1;
        self.summary.total_bytes = self.summary.total_bytes.saturating_add(size_bytes);
        Ok(())
    }
}

struct GsiFlashContext<'a, F>
where
    F: FnMut(GsiEvent),
{
    dev: &'a mut FastbootDevice,
    vars: &'a mut HashMap<String, String>,
    summary: &'a mut GsiFlashSummary,
    report: &'a mut F,
    options: &'a GsiFlashOptions,
}

impl<'a, F> GsiFlashContext<'a, F>
where
    F: FnMut(GsiEvent),
{
    async fn flash_vbmeta_logged(
        &mut self,
        capabilities: &FastbootCapabilities,
        image: &Path,
        size_bytes: u64,
    ) -> anyhow::Result<()> {
        check_cancelled(&self.options.cancel_token)?;
        (self.report)(GsiEvent::Step(GsiStep::PreparingVbmetaFlash));
        let current_slot = normalize_slot(self.vars.get("current-slot"));
        let (vbmeta_partition, vbmeta_partition_size) =
            resolve_device_partition(self.dev, self.vars, "vbmeta", current_slot.as_deref())
                .await?;
        (self.report)(GsiEvent::ResolvedPartition {
            base: "vbmeta",
            partition: vbmeta_partition.clone(),
            size_bytes: vbmeta_partition_size,
        });
        (self.report)(GsiEvent::Step(GsiStep::FlashingVbmeta));
        let mut flash = PartitionFlashContext {
            dev: self.dev,
            summary: self.summary,
            report: self.report,
            options: self.options,
        };
        flash
            .flash_partition_logged(
                &vbmeta_partition,
                image,
                size_bytes,
                capabilities.max_download_size,
            )
            .await
    }

    async fn flash_fastbootd_gsi_logged(
        &mut self,
        capabilities: &FastbootCapabilities,
        image: &Path,
        image_size: u64,
        gsi_expanded_size: u64,
        tools: &FormatTools,
    ) -> anyhow::Result<()> {
        check_cancelled(&self.options.cancel_token)?;
        (self.report)(GsiEvent::Step(GsiStep::CheckingSystemPartition));
        let current_slot = normalize_slot(self.vars.get("current-slot"));
        let (system_partition, system_partition_size) =
            resolve_device_partition(self.dev, self.vars, "system", current_slot.as_deref())
                .await?;
        (self.report)(GsiEvent::ResolvedPartition {
            base: "system",
            partition: system_partition.clone(),
            size_bytes: system_partition_size,
        });

        (self.report)(GsiEvent::Step(GsiStep::CheckingProductGsiFallback));
        if should_flash_product_gsi(system_partition_size, gsi_expanded_size) {
            check_cancelled(&self.options.cancel_token)?;
            let (product_partition, product_partition_size) =
                resolve_device_partition(self.dev, self.vars, "product", current_slot.as_deref())
                    .await?;
            (self.report)(GsiEvent::ResolvedPartition {
                base: "product",
                partition: product_partition.clone(),
                size_bytes: product_partition_size,
            });
            (self.report)(GsiEvent::Step(GsiStep::GeneratingProductGsiImage));
            let product_image = generate_product_gsi_image(tools)?;
            let product_size = std::fs::metadata(product_image.path())
                .with_context(|| {
                    format!("read image metadata for {}", product_image.path().display())
                })?
                .len();
            (self.report)(GsiEvent::Step(GsiStep::FlashingProductGsi));
            debug!(
                partition = %product_partition,
                image = %product_image.path().display(),
                size_bytes = product_size,
                "before create flash_partition_logged future"
            );
            debug!(
                partition = %product_partition,
                size_bytes = product_size,
                "after create flash_partition_logged future"
            );
            let mut flash = PartitionFlashContext {
                dev: self.dev,
                summary: self.summary,
                report: self.report,
                options: self.options,
            };
            flash
                .flash_partition_logged(
                    &product_partition,
                    product_image.path(),
                    product_size,
                    capabilities.max_download_size,
                )
                .await?;
        } else {
            (self.report)(GsiEvent::Step(GsiStep::ProductGsiFallbackNotNeeded));
        }

        check_cancelled(&self.options.cancel_token)?;
        (self.report)(GsiEvent::Step(GsiStep::FlashingSystemGsi));
        let mut flash = PartitionFlashContext {
            dev: self.dev,
            summary: self.summary,
            report: self.report,
            options: self.options,
        };
        flash
            .flash_partition_logged(
                &system_partition,
                image,
                image_size,
                capabilities.max_download_size,
            )
            .await
    }
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
            let mut flash = PartitionFlashContext {
                dev,
                summary,
                report,
                options,
            };
            flash
                .flash_partition_logged(
                    "userdata",
                    generated.path(),
                    userdata_bytes,
                    info.max_download_size
                        .and_then(|value| u32::try_from(value).ok())
                        .context("missing userdata max-download-size")?,
                )
                .await?;
            summary.wipe_count += 1;
        }
        Err(_error) if wipe_data.erase_fallback || info.fs_type.eq_ignore_ascii_case("raw") => {
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

/// Execute the full GSI flash flow: read variables, then delegate to
/// [`execute_gsi_flash_with_vars`].
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

/// Execute the full GSI flash flow, using the provided device and pre-read
/// variables. Handles mode transitions, vbmeta flashing, GSI flashing,
/// product_gsi fallback, and userdata wiping.
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
    anyhow::ensure!(
        metadata.is_file(),
        "{} is not a regular file",
        image.display()
    );

    let inspected = inspect_gsi_image(image)
        .with_context(|| format!("inspect GSI image {}", image.display()))?;
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
            {
                let mut flash = GsiFlashContext {
                    dev: &mut dev,
                    vars: &mut vars,
                    summary: &mut summary,
                    report: &mut report,
                    options,
                };
                flash
                    .flash_vbmeta_logged(&capabilities, &vbmeta_image, vbmeta_size)
                    .await?;
            }
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
            {
                let mut flash = GsiFlashContext {
                    dev: &mut dev,
                    vars: &mut vars,
                    summary: &mut summary,
                    report: &mut report,
                    options,
                };
                flash
                    .flash_fastbootd_gsi_logged(
                        &capabilities,
                        image,
                        metadata.len(),
                        inspected.expanded_size,
                        tools,
                    )
                    .await?;
            }
        }
        FastbootMode::Fastbootd => {
            report(GsiEvent::Step(GsiStep::StartingFastbootdPhase));
            check_cancelled(&options.cancel_token)?;
            {
                let mut flash = GsiFlashContext {
                    dev: &mut dev,
                    vars: &mut vars,
                    summary: &mut summary,
                    report: &mut report,
                    options,
                };
                flash
                    .flash_fastbootd_gsi_logged(
                        &capabilities,
                        image,
                        metadata.len(),
                        inspected.expanded_size,
                        tools,
                    )
                    .await?;
            }

            check_cancelled(&options.cancel_token)?;
            let transitioned =
                transition_mode(dev, vars, FastbootMode::Bootloader, &mut report).await?;
            dev = transitioned.0;
            vars = transitioned.1;
            capabilities = transitioned.2;

            check_cancelled(&options.cancel_token)?;
            {
                let mut flash = GsiFlashContext {
                    dev: &mut dev,
                    vars: &mut vars,
                    summary: &mut summary,
                    report: &mut report,
                    options,
                };
                flash
                    .flash_vbmeta_logged(&capabilities, &vbmeta_image, vbmeta_size)
                    .await?;
            }
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
    Ok(GsiFlashOutcome {
        device: dev,
        summary,
    })
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
        let available = HashSet::from([
            "system_a".to_string(),
            "system_b".to_string(),
            "system".to_string(),
        ]);

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
