#![deny(unsafe_code, missing_docs)]

//! MediaTek scatter parser and flash-plan generator.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ffi::OsStr,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use encoding_rs::{UTF_16BE, UTF_16LE, UTF_8};
use quick_xml::{events::Event, Reader};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

const VERSION: &str = "2026-05-17-single-scatter-v5-raw";
const NONE_TOKENS: &[&str] = &["", "NONE", "NULL", "N/A", "NA", "0"];
type RawLayouts = BTreeMap<String, Vec<Map<String, Value>>>;

const BOOTLOADER_CANONICAL: &[&str] = &["preloader", "lk", "loader_ext", "tee", "trustzone", "tz"];
const BOOT_CHAIN_CANONICAL: &[&str] = &[
    "boot",
    "vendor_boot",
    "init_boot",
    "dtbo",
    "vbmeta",
    "vbmeta_system",
    "vbmeta_vendor",
    "recovery",
];
const MODEM_CANONICAL: &[&str] = &[
    "md1img", "md1dsp", "md3img", "modem", "spmfw", "dpm", "pi_img",
];
const MCU_FW_CANONICAL: &[&str] = &[
    "scp",
    "sspm",
    "mcupm",
    "gz",
    "tinysys",
    "audio_dsp",
    "ccu",
    "apu",
    "vcp",
];
const ANDROID_CANONICAL: &[&str] = &[
    "super",
    "system",
    "vendor",
    "product",
    "odm",
    "system_ext",
    "vendor_dlkm",
    "odm_dlkm",
    "product_dlkm",
];
const REGIONAL_CANONICAL: &[&str] = &["logo", "tkv", "country", "cust", "oem", "csci"];
const IDENTITY_CANONICAL: &[&str] = &[
    "nvram",
    "nvdata",
    "nvcfg",
    "protect1",
    "protect2",
    "protect_f",
    "protect_s",
    "persist",
    "proinfo",
    "otp",
    "sec1",
    "nvram_backup",
];
const WIPE_CANONICAL: &[&str] = &["userdata", "metadata", "cache"];
const DANGEROUS_CANONICAL: &[&str] = &[
    "pgpt",
    "sgpt",
    "gpt",
    "mbr",
    "ebr1",
    "ebr2",
    "frp",
    "seccfg",
    "flashinfo",
    "bmtpool",
];

/// Parser and planner errors.
#[derive(Debug, thiserror::Error)]
pub enum ScatterError {
    /// The scatter path was not a regular file.
    #[error("scatter path is not a file: {0}")]
    NotFile(PathBuf),
    /// File I/O failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// XML parsing failed.
    #[error("XML parse failed: {0}")]
    Xml(String),
    /// A required or numeric field could not be normalized.
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug)]
struct ParsedRawScatter {
    general: Value,
    layouts: RawLayouts,
    warnings: Vec<String>,
    platform: Option<String>,
    project: Option<String>,
    format: String,
}

#[derive(Debug)]
struct ResolvedPathParts<'a> {
    original: &'a str,
    normalized: &'a str,
    resolved_path: Option<PathBuf>,
    resolved_via: Option<&'a str>,
    exists: Option<bool>,
    is_absolute_input: bool,
    input_style: &'a str,
    contains_parent_reference: bool,
    outside_package_root: Option<bool>,
    warning: Option<String>,
}

/// Storage layout selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum StorageSelect {
    /// Prefer UFS, then EMMC, then the first layout.
    #[default]
    Auto,
    /// Include all layouts.
    All,
    /// Select UFS only.
    Ufs,
    /// Select EMMC only.
    Emmc,
}

impl StorageSelect {
    fn as_python(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::All => "all",
            Self::Ufs => "ufs",
            Self::Emmc => "emmc",
        }
    }
}

/// Flash planning mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Reflect scatter-selected flashable partitions.
    #[default]
    DryRun,
    /// Flash only requested partitions or groups.
    Selective,
    /// Flash safe firmware and Android partitions.
    DirtyFlash,
    /// Flash safe partitions and wipe user-state partitions.
    CleanFlash,
}

impl Mode {
    fn as_python(self) -> &'static str {
        match self {
            Self::DryRun => "dry-run",
            Self::Selective => "selective",
            Self::DirtyFlash => "dirty-flash",
            Self::CleanFlash => "clean-flash",
        }
    }
}

/// Slot selection policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum SlotPolicy {
    /// Choose mode-specific default behavior.
    #[default]
    Auto,
    /// Slot A.
    A,
    /// Slot B.
    B,
    /// Active slot placeholder; live device lookup is not performed here.
    Active,
    /// Inactive slot placeholder; live device lookup is not performed here.
    Inactive,
    /// Plan both slots where possible.
    Both,
    /// Use all slot entries present in the scatter.
    AllFromScatter,
}

impl SlotPolicy {
    fn as_python(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::A => "a",
            Self::B => "b",
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Both => "both",
            Self::AllFromScatter => "all-from-scatter",
        }
    }
}

/// Image path resolution result.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResolvedPath {
    /// Original input path string.
    pub original: Option<String>,
    /// Normalized path string.
    pub normalized: Option<String>,
    /// Resolved absolute path if found.
    pub resolved_path: Option<String>,
    /// How the path was resolved (e.g. "scatter_dir", "firmware_dir", "package_root").
    pub resolved_via: Option<String>,
    /// Whether the resolved path exists on disk.
    pub exists: Option<bool>,
    /// Whether the input was an absolute path.
    pub is_absolute_input: bool,
    /// The input style (e.g. "absolute", "relative", "basename").
    pub input_style: Option<String>,
    /// Whether the path contains parent directory (`..`) references.
    pub contains_parent_reference: bool,
    /// Whether the resolved path falls outside the package root.
    pub outside_package_root: Option<bool>,
    /// Warning message about path resolution.
    pub warning: Option<String>,
}

/// One normalized scatter partition.
#[derive(Debug, Clone, Serialize)]
pub struct ScatterPartition {
    /// Source format ("xml" or "yaml").
    pub source: String,
    /// Storage layout name (e.g. "EMMC", "UFS").
    pub layout: String,
    /// Partition index string (e.g. "SYS0").
    pub index: Option<String>,
    /// Partition name (e.g. "boot", "preloader").
    pub name: String,
    /// Image file name, if any.
    pub file_name: Option<String>,
    /// Whether this partition is marked for download.
    pub is_download: bool,
    /// Image type hint (e.g. "SV5_BL_BIN").
    #[serde(rename = "type")]
    pub image_type: Option<String>,
    /// Linear start address in bytes.
    pub linear_start: i64,
    /// Physical start address in bytes.
    pub physical_start: i64,
    /// Partition size in bytes.
    pub size: i64,
    /// Storage region (e.g. "EMMC_BOOT1_BOOT2").
    pub region: String,
    /// Storage type identifier.
    pub storage: Option<String>,
    /// Whether boundary check is enabled.
    pub boundary_check: bool,
    /// Whether the partition is reserved.
    pub is_reserved: bool,
    /// Operation type hint (e.g. "BOOTLOADERS").
    pub operation_type: Option<String>,
    /// Whether the partition is upgradable.
    pub is_upgradable: Option<bool>,
    /// Whether empty boot is needed.
    pub empty_boot_needed: Option<bool>,
    /// Whether combo partition size check is enabled.
    pub combo_partsize_check: Option<bool>,
    /// Raw partition data from scatter.
    pub raw: Value,
    /// Unknown or unrecognized fields from the scatter.
    pub unknown_fields: BTreeMap<String, Value>,
}

impl ScatterPartition {
    /// End offset (linear_start + size).
    pub fn end(&self) -> i64 {
        self.linear_start + self.size
    }

    /// Base partition name without slot suffix.
    pub fn base_name(&self) -> String {
        split_base_slot(&self.name).0
    }

    /// Slot suffix if present (e.g. "_a", "_b").
    pub fn slot(&self) -> Option<String> {
        split_base_slot(&self.name).1
    }

    /// Canonical name for role/safety matching.
    pub fn canonical(&self) -> String {
        canonical_name(&self.name)
    }

    /// Region family identifier.
    pub fn region_family(&self) -> String {
        region_family(&self.region)
    }

    /// Storage family identifier derived from storage, layout, and region.
    pub fn storage_family(&self) -> String {
        storage_family(
            self.storage.as_deref(),
            Some(&self.layout),
            Some(&self.region),
        )
    }

    /// Whether this partition is flashable by profile checks.
    pub fn flashable_by_profile(&self) -> bool {
        self.is_download && self.file_name.is_some() && self.size > 0
    }

    /// Safety classification for this partition.
    pub fn safety_class(&self) -> String {
        safety_class(&self.name)
    }

    /// Role label for this partition.
    pub fn role(&self) -> String {
        role_for_name(&self.name)
    }
}

/// Parsed scatter file with all layouts.
#[derive(Debug, Clone)]
pub struct ScatterFile {
    /// Path to the scatter file on disk.
    pub path: PathBuf,
    /// Source format ("xml" or "yaml").
    pub format: String,
    /// SHA-256 hash of the raw text content.
    pub text_hash: String,
    /// Platform name from scatter metadata.
    pub platform: Option<String>,
    /// Project name from scatter metadata.
    pub project: Option<String>,
    /// Raw general section from the scatter.
    pub general: Value,
    /// Partition layouts keyed by storage type name.
    pub layouts: BTreeMap<String, Vec<ScatterPartition>>,
    /// Warnings produced during parsing.
    pub warnings: Vec<String>,
    /// Errors produced during parsing.
    pub errors: Vec<String>,
}

impl ScatterFile {
    /// Return a canonical chipset label derived from scatter metadata.
    pub fn chipset(&self) -> Option<String> {
        chipset_label(self.platform.as_deref(), self.project.as_deref())
    }

    /// Return selected layouts according to the storage policy.
    pub fn selected_layouts(
        &self,
        storage: StorageSelect,
    ) -> BTreeMap<String, Vec<ScatterPartition>> {
        if storage == StorageSelect::All {
            return self.layouts.clone();
        }

        let upper_to_key = self
            .layouts
            .keys()
            .map(|key| (key.to_uppercase(), key.clone()))
            .collect::<BTreeMap<_, _>>();

        match storage {
            StorageSelect::Ufs => upper_to_key
                .get("UFS")
                .map(|key| BTreeMap::from([(key.clone(), self.layouts[key].clone())]))
                .unwrap_or_default(),
            StorageSelect::Emmc => upper_to_key
                .get("EMMC")
                .map(|key| BTreeMap::from([(key.clone(), self.layouts[key].clone())]))
                .unwrap_or_default(),
            StorageSelect::Auto => {
                for wanted in ["UFS", "EMMC"] {
                    if let Some(key) = upper_to_key.get(wanted) {
                        return BTreeMap::from([(key.clone(), self.layouts[key].clone())]);
                    }
                }
                self.layouts
                    .iter()
                    .next()
                    .map(|(key, parts)| BTreeMap::from([(key.clone(), parts.clone())]))
                    .unwrap_or_default()
            }
            StorageSelect::All => unreachable!("handled by early return above"),
        }
    }

    fn selected_partitions(&self, storage: StorageSelect) -> Vec<ScatterPartition> {
        self.selected_layouts(storage)
            .into_values()
            .flatten()
            .collect()
    }

    /// Serialize the rich parser output.
    pub fn to_json(
        &self,
        storage: StorageSelect,
        firmware_dir: Option<&Path>,
        package_root: Option<&Path>,
        check_images: bool,
        image_search: bool,
        include_all_layouts: bool,
    ) -> Value {
        let layouts = if include_all_layouts || storage == StorageSelect::All {
            self.layouts.clone()
        } else {
            self.selected_layouts(storage)
        };
        let scatter_dir = self.path.parent();
        let layout_values = layouts
            .iter()
            .map(|(name, parts)| {
                (
                    name.clone(),
                    json!({
                        "partition_count": parts.len(),
                        "layout_hash": layout_hash(parts, false),
                        "profile_hash": layout_hash(parts, true),
                        "partitions": parts.iter().map(|part| partition_to_json(part, scatter_dir, firmware_dir, package_root, check_images, image_search)).collect::<Vec<_>>(),
                    }),
                )
            })
            .collect::<Map<_, _>>();

        json!({
            "tool": "mtk-scatter-parser",
            "version": VERSION,
            "source": self.path.to_string_lossy(),
            "format": self.format,
            "sha256_text": self.text_hash,
            "platform": self.platform,
            "project": self.project,
            "chipset": self.chipset(),
            "general": self.general,
            "storage_selection": storage.as_python(),
            "layout_names": layouts.keys().collect::<Vec<_>>(),
            "summary": {
                "layout_count": layouts.len(),
                "partition_count": layouts.values().map(Vec::len).sum::<usize>(),
                "warnings": self.warnings.len(),
                "errors": self.errors.len(),
            },
            "layouts": layout_values,
            "warnings": self.warnings,
            "errors": self.errors,
        })
    }
}

/// Offline scatter/package manifest used as the planner input.
pub type ScatterManifest = ScatterFile;

/// Flash planner options.
#[derive(Debug, Clone)]
pub struct FlashPlanOptions {
    /// Flash planning mode.
    pub mode: Mode,
    /// Storage layout selection strategy.
    pub storage: StorageSelect,
    /// Slot selection policy.
    pub slot_policy: SlotPolicy,
    /// Explicit partition names to include.
    pub parts: Vec<String>,
    /// Partition groups to include.
    pub groups: Vec<String>,
    /// Directory containing firmware images.
    pub firmware_dir: Option<PathBuf>,
    /// Package root directory for resolving image paths.
    pub package_root: Option<PathBuf>,
    /// Whether to verify image file existence and size.
    pub check_images: bool,
    /// Whether to search for images by basename.
    pub image_search: bool,
    /// Whether to include preloader in dirty-flash mode.
    pub include_preloader: bool,
    /// Whether to allow incomplete slot pairs.
    pub allow_incomplete_slots: bool,
}

/// Preview-plan options used for offline planning.
pub type PreviewPlanOptions = FlashPlanOptions;

impl Default for FlashPlanOptions {
    fn default() -> Self {
        Self {
            mode: Mode::DryRun,
            storage: StorageSelect::Auto,
            slot_policy: SlotPolicy::Auto,
            parts: Vec::new(),
            groups: Vec::new(),
            firmware_dir: None,
            package_root: None,
            check_images: false,
            image_search: false,
            include_preloader: false,
            allow_incomplete_slots: false,
        }
    }
}

/// Flash plan summary.
#[derive(Debug, Clone, Serialize, Default)]
pub struct FlashPlanSummary {
    /// Number of flash actions.
    pub flash_count: usize,
    /// Number of wipe actions.
    pub wipe_count: usize,
    /// Number of skipped partitions.
    pub skipped_count: usize,
    /// Number of actions with missing images.
    pub missing_image_count: usize,
    /// Number of actions with oversized images.
    pub oversized_image_count: usize,
    /// Total warnings across all actions.
    pub action_warning_count: usize,
    /// Number of incomplete slot base names.
    pub incomplete_slot_base_count: usize,
    /// Number of plan-level warnings.
    pub warning_count: usize,
    /// Number of plan-level errors.
    pub error_count: usize,
}

/// Execution semantics for a planned flash action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlashActionExecutionKind {
    /// Flash an image file to a partition.
    Flash,
    /// Generate and flash a formatted partition image from live device info.
    FormatData,
    /// Erase a partition only when it exists on the connected device.
    EraseIfPresent,
}

/// A flash or wipe action.
#[derive(Debug, Clone, Serialize)]
pub struct FlashAction {
    /// Action type ("flash" or "wipe").
    pub action: String,
    /// Execution semantics for the action.
    pub execution_kind: FlashActionExecutionKind,
    /// Full partition name.
    pub partition: String,
    /// Base partition name without slot suffix.
    pub base_name: String,
    /// Slot suffix if applicable.
    pub slot: Option<String>,
    /// Storage layout name.
    pub layout: String,
    /// Storage region name.
    pub region: String,
    /// Linear start address in bytes.
    pub start: i64,
    /// Linear start address as hex string.
    pub start_hex: String,
    /// Partition size in bytes.
    pub size: i64,
    /// Partition size as hex string.
    pub size_hex: String,
    /// Human-readable partition size.
    pub size_human: String,
    /// Resolved image information.
    pub image: Option<Value>,
    /// Scatter image type hint.
    pub image_type: Option<String>,
    /// Safety classification.
    pub safety_class: String,
    /// Reason for this action.
    pub reason: String,
    /// Per-action warnings.
    pub warnings: Vec<String>,
}

impl FlashAction {
    /// Return the resolved image path for flash actions, if the planner found one.
    pub fn image_resolved_path(&self) -> Option<&str> {
        self.image
            .as_ref()?
            .pointer("/path/resolved_path")?
            .as_str()
    }

    /// Return whether the resolved image exists, if that status is known.
    pub fn image_exists(&self) -> Option<bool> {
        self.image.as_ref()?.pointer("/path/exists")?.as_bool()
    }
}

/// A partition omitted from a plan.
#[derive(Debug, Clone, Serialize)]
pub struct SkippedPartition {
    /// Full partition name.
    pub partition: String,
    /// Storage layout name.
    pub layout: String,
    /// Storage region name.
    pub region: String,
    /// Reason the partition was skipped.
    pub reason: String,
    /// Safety classification.
    pub safety_class: String,
    /// Image file name, if any.
    pub file_name: Option<String>,
}

/// Planned flash operations.
#[derive(Debug, Clone, Serialize)]
pub struct FlashPlan {
    /// Effective flash mode.
    pub mode: String,
    /// Storage selection strategy used.
    pub storage_selection: String,
    /// Names of selected layouts.
    pub selected_layouts: Vec<String>,
    /// Requested slot policy.
    pub slot_policy_requested: String,
    /// Effective slot policy applied.
    pub slot_policy_effective: String,
    /// Firmware image directory.
    pub firmware_dir: Option<String>,
    /// Package root directory.
    pub package_root: Option<String>,
    /// Serialized planner options.
    pub options: Value,
    /// Plan summary counts.
    pub summary: FlashPlanSummary,
    /// Flash and wipe actions.
    pub actions: Vec<FlashAction>,
    /// Partitions skipped from the plan.
    pub skipped: Vec<SkippedPartition>,
    /// Incomplete slot bases (map of base name to details).
    pub incomplete_slots: BTreeMap<String, Value>,
    /// Plan-level warnings.
    pub warnings: Vec<String>,
    /// Plan-level errors.
    pub errors: Vec<String>,
}

/// Offline preview plan derived from a scatter manifest.
pub type PreviewPlan = FlashPlan;

/// Load and normalize a scatter manifest from disk.
pub fn load_scatter_manifest(path: impl AsRef<Path>) -> Result<ScatterManifest, ScatterError> {
    parse_scatter(path)
}

/// Build an offline preview plan from a parsed scatter manifest.
pub fn build_preview_plan(scatter: &ScatterManifest, options: PreviewPlanOptions) -> PreviewPlan {
    build_flash_plan(scatter, options)
}

/// Parse a MediaTek scatter file.
pub fn parse_scatter(path: impl AsRef<Path>) -> Result<ScatterFile, ScatterError> {
    let path = path.as_ref();
    if !path.is_file() {
        return Err(ScatterError::NotFile(path.to_path_buf()));
    }
    let text = decode_text(path)?;
    let text_hash = sha256_text(&text);
    let parsed = if looks_like_xml(&text) {
        parse_xml_scatter(&text)?
    } else {
        let (general, layouts, warnings, platform, project) = parse_yaml_scatter(&text);
        ParsedRawScatter {
            general,
            layouts,
            warnings,
            platform,
            project,
            format: "yaml".to_string(),
        }
    };
    let mut warnings = parsed.warnings;

    let mut layouts = BTreeMap::new();
    let mut errors = Vec::new();
    for (layout, entries) in parsed.layouts {
        let norm_layout = if layout.trim().is_empty() {
            "DEFAULT".to_string()
        } else {
            layout.trim().to_string()
        };
        let mut parts = Vec::new();
        for entry in entries {
            match normalize_partition(path, &norm_layout, entry.clone()) {
                Ok(part) => parts.push(part),
                Err(err) => errors.push(format!(
                    "{norm_layout}: failed to normalize partition entry {entry:?}: {err}"
                )),
            }
        }
        layouts.insert(norm_layout, parts);
    }
    validate_layouts(&layouts, &mut warnings, &mut errors);

    Ok(ScatterFile {
        path: path.to_path_buf(),
        format: parsed.format,
        text_hash,
        platform: parsed.platform,
        project: parsed.project,
        general: parsed.general,
        layouts,
        warnings,
        errors,
    })
}

/// Build a safe flash plan for a parsed scatter file.
pub fn build_flash_plan(scatter: &ScatterFile, options: FlashPlanOptions) -> FlashPlan {
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    record_unknown_groups(&options.groups, &mut errors);

    let selected_parts = scatter.selected_partitions(options.storage);
    let parts_by_name = selected_parts
        .iter()
        .map(|part| (part.name.to_lowercase(), part))
        .collect::<BTreeMap<_, _>>();
    let available_names = selected_parts
        .iter()
        .map(|part| part.name.to_lowercase())
        .collect::<BTreeSet<_>>();
    let slot_policy_eff = resolve_effective_slot_policy(options.mode, options.slot_policy);
    let explicit_names = expand_requested_names(&options.parts, &available_names, slot_policy_eff);

    let scatter_dir = scatter.path.parent();
    let mut actions = Vec::new();
    let mut skipped = Vec::new();
    warn_for_placeholder_slot_policy(slot_policy_eff, &mut warnings);

    for part in &selected_parts {
        let (selected, selection_reason) =
            select_partition_for_mode(part, &options, &explicit_names);

        if !selected {
            skipped.push(skipped_partition(part, "not selected"));
            continue;
        }

        if let Some(reason) = skip_reason_for_slot_policy(part, slot_policy_eff) {
            skipped.push(skipped_partition(part, &reason));
            continue;
        }

        let image_source = inherited_image_source_for_slot_b(part, &parts_by_name, slot_policy_eff);
        let (allowed, reason) =
            mode_allows_partition(part, image_source, options.mode, options.include_preloader);
        if !allowed {
            skipped.push(skipped_partition(part, &reason));
            continue;
        }

        let (image, action_warnings) = resolve_images_for_plan(image_source, scatter_dir, &options);
        let action_reason = if selection_reason.is_empty() {
            inherited_action_reason(reason, part, image_source)
        } else {
            inherited_action_reason(selection_reason, part, image_source)
        };

        actions.push(flash_action(
            "flash",
            part,
            Some(image),
            &action_reason,
            action_warnings,
        ));
    }

    append_clean_flash_wipes(
        &selected_parts,
        scatter_dir,
        &options,
        options.mode,
        &mut actions,
    );
    let existing_wipes = actions
        .iter()
        .filter(|action| WIPE_CANONICAL.contains(&canonical_name(&action.partition).as_str()))
        .map(|action| canonical_name(&action.partition))
        .collect::<BTreeSet<_>>();
    append_missing_clean_flash_wipes(options.mode, &existing_wipes, &mut actions);

    warn_for_missing_selective_requests(
        options.mode,
        &actions,
        &explicit_names,
        &available_names,
        &mut warnings,
    );

    synthesize_slot_actions_if_needed(slot_policy_eff, options.mode, &selected_parts, &mut actions);

    let incomplete_slots = check_incomplete_slots(
        &selected_parts,
        &actions,
        slot_policy_eff,
        options.allow_incomplete_slots,
        &mut warnings,
        &mut errors,
    );

    let missing_images = actions
        .iter()
        .filter(|action| {
            action.action == "flash"
                && action
                    .image
                    .as_ref()
                    .and_then(|image| image.pointer("/path/exists"))
                    == Some(&Value::Bool(false))
        })
        .count();
    let oversized_images = actions
        .iter()
        .filter(|action| {
            action.action == "flash"
                && action
                    .image
                    .as_ref()
                    .and_then(|image| image.pointer("/status/fits_partition"))
                    == Some(&Value::Bool(false))
        })
        .count();
    let action_warning_count = actions
        .iter()
        .map(|action| action.warnings.len())
        .sum::<usize>();
    if options.check_images && missing_images > 0 {
        errors.push(format!("missing images: {missing_images}"));
    }
    if options.check_images && oversized_images > 0 {
        errors.push(format!("oversized images: {oversized_images}"));
    }

    let summary = finalize_plan_summary(
        &actions,
        PlanSummaryCounts {
            skipped_count: skipped.len(),
            missing_image_count: missing_images,
            oversized_image_count: oversized_images,
            action_warning_count,
            incomplete_slot_base_count: incomplete_slots.len(),
            warning_count: warnings.len(),
            error_count: errors.len(),
        },
    );

    FlashPlan {
        mode: options.mode.as_python().to_string(),
        storage_selection: options.storage.as_python().to_string(),
        selected_layouts: scatter
            .selected_layouts(options.storage)
            .keys()
            .cloned()
            .collect(),
        slot_policy_requested: options.slot_policy.as_python().to_string(),
        slot_policy_effective: slot_policy_eff.as_python().to_string(),
        firmware_dir: options
            .firmware_dir
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        package_root: options
            .package_root
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        options: json!({
            "check_images": options.check_images,
            "image_search": options.image_search,
            "include_preloader": options.include_preloader,
            "allow_incomplete_slots": options.allow_incomplete_slots,
            "parts": options.parts,
            "groups": options.groups,
        }),
        summary,
        actions,
        skipped,
        incomplete_slots,
        warnings,
        errors,
    }
}

fn record_unknown_groups(groups: &[String], errors: &mut Vec<String>) {
    let known_groups = group_names();
    let unknown_groups = groups
        .iter()
        .filter(|group| !known_groups.contains(group.to_lowercase().as_str()))
        .cloned()
        .collect::<BTreeSet<_>>();
    for group in unknown_groups {
        errors.push(format!("unknown group: {group}"));
    }
}

fn resolve_effective_slot_policy(mode: Mode, requested: SlotPolicy) -> SlotPolicy {
    // Dry-run is a preview of the same flash plan, so keep slot synthesis
    // aligned with live flashing modes. Only selective mode preserves the
    // scatter-defined slot set.
    match requested {
        SlotPolicy::Auto if matches!(mode, Mode::Selective) => SlotPolicy::AllFromScatter,
        SlotPolicy::Auto => SlotPolicy::Both,
        other => other,
    }
}

fn warn_for_placeholder_slot_policy(slot_policy: SlotPolicy, warnings: &mut Vec<String>) {
    if matches!(slot_policy, SlotPolicy::Active | SlotPolicy::Inactive) {
        warnings.push(format!(
            "slot policy {:?} needs live device active-slot info; treating as all-from-scatter",
            slot_policy.as_python()
        ));
    }
}

fn select_partition_for_mode(
    part: &ScatterPartition,
    options: &FlashPlanOptions,
    explicit_names: &BTreeSet<String>,
) -> (bool, String) {
    match options.mode {
        Mode::DryRun | Mode::DirtyFlash | Mode::CleanFlash => {
            (true, format!("mode {}", options.mode.as_python()))
        }
        Mode::Selective => {
            let by_part = explicit_names.contains(&part.name.to_lowercase())
                || explicit_names.contains(&part.base_name().to_lowercase())
                || explicit_names.contains(&part.canonical());
            let by_group = part_matches_group(part, &options.groups);
            let reason = match (by_part, by_group) {
                (true, true) => "selected by part and group",
                (true, false) => "selected by part",
                (false, true) => "selected by group",
                (false, false) => "",
            };
            (by_part || by_group, reason.to_string())
        }
    }
}

fn skip_reason_for_slot_policy(part: &ScatterPartition, slot_policy: SlotPolicy) -> Option<String> {
    if !matches!(slot_policy, SlotPolicy::A | SlotPolicy::B) {
        return None;
    }
    let wanted = if slot_policy == SlotPolicy::A {
        "a"
    } else {
        "b"
    };
    part.slot()
        .as_deref()
        .filter(|slot| *slot != wanted)
        .map(|_| format!("slot policy {wanted}"))
}

fn resolve_images_for_plan(
    part: &ScatterPartition,
    scatter_dir: Option<&Path>,
    options: &FlashPlanOptions,
) -> (Value, Vec<String>) {
    let resolved = resolve_image_path(
        part.file_name.as_deref(),
        scatter_dir,
        options.firmware_dir.as_deref(),
        options.package_root.as_deref(),
        options.image_search,
    );
    let (status, mut warnings) = checked_image_status(
        resolved.resolved_path.as_deref(),
        resolved.exists,
        options.check_images,
        part.size,
    );
    if let Some(warning) = &resolved.warning {
        warnings.insert(0, warning.clone());
    }
    (
        json!({
            "file_name": part.file_name,
            "path": resolved,
            "status": status,
        }),
        warnings,
    )
}

fn checked_image_status(
    resolved_path: Option<&str>,
    exists: Option<bool>,
    checked: bool,
    target_size: i64,
) -> (Value, Vec<String>) {
    let mut warnings = Vec::new();
    let mut status = json!({
        "checked": checked,
        "exists": exists,
        "size_bytes": null,
        "size_human": null,
        "fits_partition": null,
        "magic": null,
    });
    if !checked {
        return (status, warnings);
    }

    if let Some(path) = resolved_path.filter(|_| exists == Some(true)) {
        match fs::metadata(path) {
            Ok(meta) => {
                let size = i64::try_from(meta.len()).unwrap_or(i64::MAX);
                status["size_bytes"] = json!(size);
                status["size_human"] = json!(human_size(size));
                status["fits_partition"] = json!(size <= target_size);
                status["magic"] = json!(image_magic(Path::new(path)));
                if size > target_size {
                    warnings.push(format!(
                        "image is larger than partition: {size} > {target_size}"
                    ));
                }
            }
            Err(err) => warnings.push(format!("failed to stat image: {err}")),
        }
    } else {
        warnings.push("image missing".to_string());
    }
    (status, warnings)
}

fn append_clean_flash_wipes(
    selected_parts: &[ScatterPartition],
    scatter_dir: Option<&Path>,
    options: &FlashPlanOptions,
    mode: Mode,
    actions: &mut Vec<FlashAction>,
) {
    if !matches!(mode, Mode::CleanFlash | Mode::DryRun) {
        return;
    }
    for canonical in ["userdata", "cache", "metadata"] {
        let mut matched = false;
        for part in selected_parts
            .iter()
            .filter(|part| part.canonical() == canonical)
        {
            matched = true;
            if part.canonical() == "userdata" {
                let (image, warnings) = resolve_images_for_plan(part, scatter_dir, options);
                let image_exists = image
                    .pointer("/path/exists")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if image_exists {
                    actions.push(flash_action(
                        "flash",
                        part,
                        Some(image),
                        "clean-flash resets userdata using bundled image before live format",
                        warnings,
                    ));
                }
                actions.push(flash_action(
                    "wipe",
                    part,
                    None,
                    if image_exists {
                        "clean-flash formats userdata using live device partition info after flashing bundled userdata image"
                    } else {
                        "clean-flash formats userdata using live device partition info"
                    },
                    Vec::new(),
                ));
            } else {
                let image = part
                    .file_name
                    .as_ref()
                    .map(|file_name| json!({ "file_name": file_name }));
                actions.push(flash_action(
                    "wipe",
                    part,
                    image,
                    if part.canonical() == "metadata" {
                        "clean-flash formats metadata using live device partition info"
                    } else {
                        "clean-flash wipes user state if present on connected device"
                    },
                    Vec::new(),
                ));
            }
        }
        if !matched && mode == Mode::CleanFlash {
            actions.push(synthetic_clean_flash_wipe(canonical));
        }
    }
}

fn append_missing_clean_flash_wipes(
    mode: Mode,
    existing_wipes: &BTreeSet<String>,
    actions: &mut Vec<FlashAction>,
) {
    if mode != Mode::CleanFlash {
        return;
    }

    for partition in ["userdata", "cache", "metadata"] {
        if existing_wipes.contains(partition) {
            continue;
        }
        actions.push(synthetic_clean_flash_wipe(partition));
    }
}

fn synthetic_clean_flash_wipe(partition: &str) -> FlashAction {
    FlashAction {
        action: "wipe".to_string(),
        execution_kind: if matches!(partition, "userdata" | "metadata") {
            FlashActionExecutionKind::FormatData
        } else {
            FlashActionExecutionKind::EraseIfPresent
        },
        partition: partition.to_string(),
        base_name: partition.to_string(),
        slot: None,
        layout: "SYNTHETIC".to_string(),
        region: "SYNTHETIC".to_string(),
        start: 0,
        start_hex: "0x0".to_string(),
        size: 1,
        size_hex: "0x1".to_string(),
        size_human: human_size(1),
        image: None,
        image_type: None,
        safety_class: safety_class(partition),
        reason: match partition {
            "userdata" => {
                "clean-flash formats userdata when no bundled image is available".to_string()
            }
            "metadata" => {
                "clean-flash formats metadata using live device partition info".to_string()
            }
            _ => "clean-flash wipes user state if present on connected device".to_string(),
        },
        warnings: Vec::new(),
    }
}

fn warn_for_missing_selective_requests(
    mode: Mode,
    actions: &[FlashAction],
    explicit_names: &BTreeSet<String>,
    available_names: &BTreeSet<String>,
    warnings: &mut Vec<String>,
) {
    if mode != Mode::Selective {
        return;
    }
    let planned_names = actions
        .iter()
        .map(|action| action.partition.to_lowercase())
        .collect::<BTreeSet<_>>();
    for req in explicit_names {
        if !available_names.contains(req) && !planned_names.contains(req) {
            warnings.push(format!(
                "requested partition not found in selected layout: {req}"
            ));
        }
    }
}

fn synthesize_slot_actions_if_needed(
    slot_policy: SlotPolicy,
    mode: Mode,
    selected_parts: &[ScatterPartition],
    actions: &mut Vec<FlashAction>,
) {
    if slot_policy == SlotPolicy::Both && mode != Mode::Selective {
        synthesize_non_download_slot_actions(selected_parts, actions);
    }
}

fn check_incomplete_slots(
    selected_parts: &[ScatterPartition],
    actions: &[FlashAction],
    slot_policy: SlotPolicy,
    allow_incomplete_slots: bool,
    warnings: &mut Vec<String>,
    errors: &mut Vec<String>,
) -> BTreeMap<String, Value> {
    let mut incomplete_slots = BTreeMap::new();
    if slot_policy != SlotPolicy::Both {
        return incomplete_slots;
    }

    let mut by_base_available: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut by_base_planned: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for part in selected_parts {
        if let Some(slot) = part.slot() {
            by_base_available
                .entry(part.base_name())
                .or_default()
                .insert(slot);
        }
    }
    for action in actions.iter().filter(|action| action.action == "flash") {
        if let Some(slot) = &action.slot {
            by_base_planned
                .entry(action.base_name.clone())
                .or_default()
                .insert(slot.clone());
        }
    }
    for (base, available) in by_base_available {
        if !available.is_superset(&BTreeSet::from(["a".to_string(), "b".to_string()])) {
            continue;
        }
        let planned = by_base_planned.get(&base).cloned().unwrap_or_default();
        if !planned.is_empty() && planned != BTreeSet::from(["a".to_string(), "b".to_string()]) {
            let available_slots = available.iter().cloned().collect::<Vec<_>>();
            let planned_slots = planned.iter().cloned().collect::<Vec<_>>();
            incomplete_slots.insert(
                base.clone(),
                json!({
                    "available_slots": available_slots,
                    "planned_slots": planned_slots,
                }),
            );
            let message = format!(
                "slot policy both requested but only planned slots {planned_slots:?} for {base}; available slots are {available_slots:?}"
            );
            if allow_incomplete_slots {
                warnings.push(message);
            } else {
                errors.push(message);
            }
        }
    }
    incomplete_slots
}

struct PlanSummaryCounts {
    skipped_count: usize,
    missing_image_count: usize,
    oversized_image_count: usize,
    action_warning_count: usize,
    incomplete_slot_base_count: usize,
    warning_count: usize,
    error_count: usize,
}

fn finalize_plan_summary(actions: &[FlashAction], counts: PlanSummaryCounts) -> FlashPlanSummary {
    FlashPlanSummary {
        flash_count: actions
            .iter()
            .filter(|action| action.action == "flash")
            .count(),
        wipe_count: actions
            .iter()
            .filter(|action| action.action == "wipe")
            .count(),
        skipped_count: counts.skipped_count,
        missing_image_count: counts.missing_image_count,
        oversized_image_count: counts.oversized_image_count,
        action_warning_count: counts.action_warning_count,
        incomplete_slot_base_count: counts.incomplete_slot_base_count,
        warning_count: counts.warning_count,
        error_count: counts.error_count,
    }
}

fn synthesize_non_download_slot_actions(
    selected_parts: &[ScatterPartition],
    actions: &mut Vec<FlashAction>,
) {
    let parts_by_name = selected_parts
        .iter()
        .map(|part| (part.name.to_lowercase(), part))
        .collect::<BTreeMap<_, _>>();
    let actions_by_partition = actions
        .iter()
        .filter(|action| action.action == "flash")
        .map(|action| (action.partition.to_lowercase(), action.clone()))
        .collect::<BTreeMap<_, _>>();
    let planned = actions_by_partition
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut synthesized = Vec::new();

    for source_action in actions_by_partition.values() {
        let Some(source_slot) = source_action.slot.as_deref() else {
            continue;
        };
        let target_slot = match source_slot {
            "a" => "b",
            "b" => "a",
            _ => continue,
        };
        let target_name = format!("{}_{}", source_action.base_name, target_slot);
        if planned.contains(&target_name) {
            continue;
        }
        let Some(target_part) = parts_by_name.get(&target_name) else {
            continue;
        };
        if target_part.flashable_by_profile() {
            continue;
        }
        synthesized.push(slot_synthesized_action(
            source_action,
            target_part,
            source_slot,
        ));
    }

    actions.extend(synthesized);
}

fn slot_synthesized_action(
    source: &FlashAction,
    target: &ScatterPartition,
    source_slot: &str,
) -> FlashAction {
    let (image, warnings) = recheck_synthesized_image(source.image.clone(), target);
    FlashAction {
        action: source.action.clone(),
        execution_kind: source.execution_kind,
        partition: target.name.clone(),
        base_name: target.base_name(),
        slot: target.slot(),
        layout: target.layout.clone(),
        region: target.region.clone(),
        start: target.linear_start,
        start_hex: format!("{:#x}", target.linear_start),
        size: target.size,
        size_hex: format!("{:#x}", target.size),
        size_human: human_size(target.size),
        image,
        image_type: target.image_type.clone(),
        safety_class: target.safety_class(),
        reason: format!("inferred from slot {source_slot} image for slot all"),
        warnings,
    }
}

fn recheck_synthesized_image(
    image: Option<Value>,
    target: &ScatterPartition,
) -> (Option<Value>, Vec<String>) {
    let Some(mut image) = image else {
        return (None, Vec::new());
    };
    let mut warnings = Vec::new();
    if let Some(warning) = image.pointer("/path/warning").and_then(Value::as_str) {
        warnings.push(warning.to_string());
    }
    let checked = image
        .pointer("/status/checked")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !checked {
        return (Some(image), warnings);
    }

    let (status, mut status_warnings) = checked_image_status(
        image.pointer("/path/resolved_path").and_then(Value::as_str),
        image.pointer("/path/exists").and_then(Value::as_bool),
        true,
        target.size,
    );
    warnings.append(&mut status_warnings);
    image["status"] = status;
    (Some(image), warnings)
}

/// Parse an integer using scatter conventions.
pub fn parse_int(value: impl ToString, field_name: &str) -> Result<i64, ScatterError> {
    let mut s = value.to_string().trim().replace('_', "");
    if s.is_empty() {
        return Err(ScatterError::Invalid(format!("empty {field_name}")));
    }
    let sign = if let Some(rest) = s.strip_prefix('-') {
        s = rest.to_string();
        -1
    } else if let Some(rest) = s.strip_prefix('+') {
        s = rest.to_string();
        1
    } else {
        1
    };

    let parsed = if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        i64::from_str_radix(rest, 16)
    } else if let Some(rest) = s.strip_suffix('h').or_else(|| s.strip_suffix('H')) {
        i64::from_str_radix(rest, 16)
    } else if s.chars().all(|ch| ch.is_ascii_digit()) {
        s.parse::<i64>()
    } else if s.chars().all(|ch| ch.is_ascii_hexdigit())
        && s.chars()
            .any(|ch| ch.is_ascii_hexdigit() && ch.is_ascii_alphabetic())
    {
        i64::from_str_radix(&s, 16)
    } else {
        return Err(ScatterError::Invalid(format!(
            "invalid {field_name}: {value}",
            value = value.to_string()
        )));
    };
    parsed.map(|num| num * sign).map_err(|_| {
        ScatterError::Invalid(format!(
            "invalid {field_name}: {value}",
            value = value.to_string()
        ))
    })
}

/// Format byte sizes like the Python parser.
pub fn human_size(num: i64) -> String {
    let mut n = num as f64;
    for unit in ["B", "KiB", "MiB", "GiB", "TiB"] {
        if n.abs() < 1024.0 || unit == "TiB" {
            if unit == "B" {
                return format!("{} B", n as i64);
            }
            return format!("{n:.2} {unit}");
        }
        n /= 1024.0;
    }
    format!("{num} B")
}

/// Canonicalize a partition name for role/safety matching.
pub fn canonical_name(name: &str) -> String {
    let (mut base, _) = split_base_slot(&name.to_lowercase());
    base = base.trim().to_string();
    if matches_numbered(&base, "tee") {
        return "tee".to_string();
    }
    if matches_numbered(&base, "lk") {
        return "lk".to_string();
    }
    if base.starts_with("preloader") {
        return "preloader".to_string();
    }
    if base.starts_with("loader_ext") {
        return "loader_ext".to_string();
    }
    if base == "tee_a" || base == "tee_b" {
        return "tee".to_string();
    }
    if is_numbered_vbmeta(&base) {
        if base.contains("system") {
            return "vbmeta_system".to_string();
        }
        if base.contains("vendor") {
            return "vbmeta_vendor".to_string();
        }
        return "vbmeta".to_string();
    }
    base
}

/// Return a safety class for a partition name.
pub fn safety_class(name: &str) -> String {
    let canonical = canonical_name(name);
    if IDENTITY_CANONICAL.contains(&canonical.as_str()) {
        "identity_or_calibration"
    } else if WIPE_CANONICAL.contains(&canonical.as_str()) {
        "wipe_only"
    } else if DANGEROUS_CANONICAL.contains(&canonical.as_str()) {
        "dangerous"
    } else if BOOTLOADER_CANONICAL.contains(&canonical.as_str()) {
        "bootloader_critical"
    } else if BOOT_CHAIN_CANONICAL.contains(&canonical.as_str()) {
        "boot_critical"
    } else if MODEM_CANONICAL.contains(&canonical.as_str())
        || MCU_FW_CANONICAL.contains(&canonical.as_str())
    {
        "firmware"
    } else if ANDROID_CANONICAL.contains(&canonical.as_str()) {
        "android_system"
    } else if REGIONAL_CANONICAL.contains(&canonical.as_str()) {
        "regional"
    } else if matches!(
        canonical.as_str(),
        "super"
            | "system_ext"
            | "vendor_dlkm"
            | "odm_dlkm"
            | "my_product"
            | "my_region"
            | "product"
            | "vendor"
            | "odm"
            | "cache"
            | "metadata"
    ) || canonical.starts_with("system")
        || canonical.starts_with("product")
        || canonical.starts_with("vendor")
        || canonical.starts_with("odm")
    {
        "android_system"
    } else if canonical.contains("vbmeta")
        || canonical.contains("boot")
        || canonical.contains("dtbo")
        || canonical.contains("recovery")
        || canonical.contains("init_boot")
    {
        "boot_critical"
    } else if canonical.contains("logo")
        || canonical.contains("splash")
        || canonical.contains("cust")
    {
        "regional"
    } else if canonical.contains("modem")
        || canonical.contains("radio")
        || canonical.contains("dsp")
        || canonical.ends_with("_fw")
    {
        "firmware"
    } else {
        "unknown"
    }
    .to_string()
}

fn role_for_name(name: &str) -> String {
    let canonical = canonical_name(name);
    if IDENTITY_CANONICAL.contains(&canonical.as_str()) {
        "identity_or_calibration"
    } else if WIPE_CANONICAL.contains(&canonical.as_str()) {
        "wipe_only"
    } else if DANGEROUS_CANONICAL.contains(&canonical.as_str()) {
        "dangerous"
    } else if BOOTLOADER_CANONICAL.contains(&canonical.as_str()) {
        "bootloader_critical"
    } else if BOOT_CHAIN_CANONICAL.contains(&canonical.as_str()) {
        "boot_chain_or_avb"
    } else if MODEM_CANONICAL.contains(&canonical.as_str()) {
        "modem_firmware"
    } else if MCU_FW_CANONICAL.contains(&canonical.as_str()) {
        "mcu_firmware"
    } else if ANDROID_CANONICAL.contains(&canonical.as_str()) {
        "android_dynamic_or_system"
    } else if REGIONAL_CANONICAL.contains(&canonical.as_str()) {
        "regional_or_branding"
    } else {
        "unknown"
    }
    .to_string()
}

fn decode_text(path: &Path) -> Result<String, ScatterError> {
    let raw = fs::read(path)?;
    let candidates = [
        UTF_8.decode(&raw).0.into_owned(),
        UTF_16LE.decode(&raw).0.into_owned(),
        UTF_16BE.decode(&raw).0.into_owned(),
        raw.iter().map(|&byte| char::from(byte)).collect::<String>(),
    ];
    for text in candidates {
        if text.matches('\0').count() < std::cmp::max(1, text.len() / 20) {
            return Ok(text.replace("\r\n", "\n").replace('\r', "\n"));
        }
    }
    Ok(String::from_utf8_lossy(&raw)
        .replace("\r\n", "\n")
        .replace('\r', "\n"))
}

fn looks_like_xml(text: &str) -> bool {
    let lower = text
        .trim_start_matches(['\u{feff}', '\n', '\r', '\t', ' '])
        .chars()
        .take(300)
        .collect::<String>()
        .to_lowercase();
    lower.starts_with("<?xml")
        || lower.starts_with("<root")
        || lower.starts_with("<scatter")
        || lower.starts_with("<da")
}

fn sha256_text(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn parse_yaml_scatter(
    text: &str,
) -> (
    Value,
    RawLayouts,
    Vec<String>,
    Option<String>,
    Option<String>,
) {
    let records = load_yaml_records(text);
    let mut general = Map::new();
    let mut layouts: RawLayouts = BTreeMap::new();
    let mut warnings = Vec::new();

    for rec in records {
        if rec.contains_key("storage_type") && rec.contains_key("description") {
            let layout =
                value_to_string(rec.get("storage_type")).unwrap_or_else(|| "UNKNOWN".to_string());
            if let Some(Value::Array(items)) = rec.get("description") {
                for item in items.iter().filter_map(Value::as_object) {
                    if item.contains_key("general")
                        || item.contains_key("config_version")
                        || item.contains_key("platform")
                        || item.contains_key("project")
                    {
                        if general.is_empty() {
                            general.extend(item.clone());
                        } else {
                            general.entry("layout_general").or_insert_with(|| json!({}));
                        }
                    }
                    if item.contains_key("partition_name") || item.contains_key("partition_index") {
                        layouts
                            .entry(layout.clone())
                            .or_default()
                            .push(item.clone());
                    }
                }
            }
            continue;
        }

        if rec.contains_key("general")
            || rec.contains_key("config_version")
            || rec.contains_key("platform")
            || rec.contains_key("project")
        {
            for (key, value) in rec {
                general.entry(key).or_insert(value);
            }
            continue;
        }

        if rec.contains_key("partition_name") || rec.contains_key("partition_index") {
            let layout = value_to_string(
                rec.get("storage_type")
                    .or_else(|| rec.get("layout"))
                    .or_else(|| rec.get("storage")),
            )
            .unwrap_or_else(|| "DEFAULT".to_string());
            layouts.entry(layout).or_default().push(rec);
        }
    }
    if layouts.is_empty() && !general.is_empty() {
        warnings.push("no partition entries found in YAML-style scatter".to_string());
    }
    let general_value = Value::Object(general);
    let platform = find_general_value(&general_value, "platform");
    let project = find_general_value(&general_value, "project");
    (general_value, layouts, warnings, platform, project)
}

fn load_yaml_records(text: &str) -> Vec<Map<String, Value>> {
    if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(text) {
        if let Ok(json_value) = serde_json::to_value(value) {
            return match json_value {
                Value::Array(items) => items
                    .into_iter()
                    .filter_map(|item| item.as_object().cloned())
                    .collect(),
                Value::Object(map) => vec![map],
                _ => Vec::new(),
            };
        }
    }
    loose_yaml_records(text)
}

fn loose_yaml_records(text: &str) -> Vec<Map<String, Value>> {
    let mut records = Vec::new();
    let mut current: Option<Map<String, Value>> = None;
    for raw_line in text.lines() {
        let mut line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('-') {
            if let Some(record) = current.take().filter(|record| !record.is_empty()) {
                records.push(record);
            }
            current = Some(Map::new());
            line = rest.trim();
        }
        let Some(record) = current.as_mut() else {
            continue;
        };
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if !key.is_empty() {
            record.insert(key.to_string(), scalar_json(value.trim()));
        }
    }
    if let Some(record) = current.filter(|record| !record.is_empty()) {
        records.push(record);
    }
    records
}

#[derive(Debug, Clone, Default)]
struct XmlNode {
    tag: String,
    attrs: Map<String, Value>,
    text: String,
    children: Vec<XmlNode>,
}

fn parse_xml_scatter(text: &str) -> Result<ParsedRawScatter, ScatterError> {
    let root = parse_xml_node(text)?;
    if matches!(
        root.tag.to_lowercase().as_str(),
        "scatter" | "checksum" | "scatter_checksum"
    ) && !root
        .descendants()
        .iter()
        .any(|node| node.tag == "partition_index")
    {
        return Ok(ParsedRawScatter {
            general: json!({}),
            layouts: BTreeMap::new(),
            warnings: Vec::new(),
            platform: None,
            project: None,
            format: "checksum_xml".to_string(),
        });
    }

    let mut general = Map::new();
    if let Some(general_node) = root
        .descendants()
        .into_iter()
        .find(|node| node.tag == "general")
    {
        general.extend(xml_children_dict(general_node));
        for (key, value) in &general_node.attrs {
            general
                .entry(format!("@{key}"))
                .or_insert_with(|| value.clone());
        }
    }
    for (key, value) in &root.attrs {
        general.entry(key.clone()).or_insert_with(|| value.clone());
    }

    let mut layouts: RawLayouts = BTreeMap::new();
    for storage_node in root
        .descendants()
        .into_iter()
        .filter(|node| node.tag == "storage_type")
    {
        let layout = value_to_string(
            storage_node
                .attrs
                .get("name")
                .or_else(|| storage_node.attrs.get("value")),
        )
        .or_else(|| {
            let text = storage_node.text.trim();
            (!text.is_empty()).then(|| text.to_string())
        })
        .unwrap_or_else(|| "UNKNOWN".to_string());
        for part_node in storage_node
            .descendants()
            .into_iter()
            .filter(|node| node.tag == "partition_index")
        {
            let mut entry = xml_children_dict(part_node);
            let index = value_to_string(
                part_node
                    .attrs
                    .get("name")
                    .or_else(|| part_node.attrs.get("value")),
            )
            .or_else(|| value_to_string(entry.get("partition_index")));
            if let Some(index) = index {
                entry.insert("partition_index".to_string(), Value::String(index));
            }
            layouts.entry(layout.clone()).or_default().push(entry);
        }
    }

    if layouts.is_empty() {
        let direct_parts = root
            .children
            .iter()
            .filter(|node| node.tag == "partition_index")
            .map(|node| {
                let mut entry = xml_children_dict(node);
                if let Some(index) =
                    value_to_string(node.attrs.get("name").or_else(|| node.attrs.get("value")))
                {
                    entry.insert("partition_index".to_string(), Value::String(index));
                }
                entry
            })
            .collect::<Vec<_>>();
        if !direct_parts.is_empty() {
            let joined = direct_parts
                .iter()
                .map(|entry| {
                    format!(
                        "{} {}",
                        value_to_string(entry.get("storage")).unwrap_or_default(),
                        value_to_string(entry.get("region")).unwrap_or_default()
                    )
                })
                .collect::<String>()
                .to_uppercase();
            layouts.insert(
                if joined.contains("UFS") {
                    "UFS"
                } else {
                    "EMMC"
                }
                .to_string(),
                direct_parts,
            );
        }
    }

    if layouts.is_empty() {
        let all_parts = root
            .descendants()
            .into_iter()
            .filter(|node| node.tag == "partition_index")
            .map(|node| {
                let mut entry = xml_children_dict(node);
                if let Some(index) =
                    value_to_string(node.attrs.get("name").or_else(|| node.attrs.get("value")))
                {
                    entry.insert("partition_index".to_string(), Value::String(index));
                }
                entry
            })
            .collect::<Vec<_>>();
        if !all_parts.is_empty() {
            layouts.insert("DEFAULT".to_string(), all_parts);
        }
    }

    let general_value = Value::Object(general);
    let platform = find_general_value(&general_value, "platform");
    let project = find_general_value(&general_value, "project");
    Ok(ParsedRawScatter {
        general: general_value,
        layouts,
        warnings: Vec::new(),
        platform,
        project,
        format: "xml".to_string(),
    })
}

impl XmlNode {
    fn descendants(&self) -> Vec<&XmlNode> {
        let mut out = vec![self];
        for child in &self.children {
            out.extend(child.descendants());
        }
        out
    }
}

fn parse_xml_node(text: &str) -> Result<XmlNode, ScatterError> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut stack: Vec<XmlNode> = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(start)) => {
                let tag = strip_ns(std::str::from_utf8(start.name().as_ref()).unwrap_or_default());
                let mut attrs = Map::new();
                for attr in start.attributes().flatten() {
                    let key = strip_ns(std::str::from_utf8(attr.key.as_ref()).unwrap_or_default());
                    let value = attr
                        .unescape_value()
                        .map_or(Value::Null, |value| scalar_json(value.as_ref()));
                    attrs.insert(key, value);
                }
                stack.push(XmlNode {
                    tag,
                    attrs,
                    text: String::new(),
                    children: Vec::new(),
                });
            }
            Ok(Event::Empty(empty)) => {
                let tag = strip_ns(std::str::from_utf8(empty.name().as_ref()).unwrap_or_default());
                let mut attrs = Map::new();
                for attr in empty.attributes().flatten() {
                    let key = strip_ns(std::str::from_utf8(attr.key.as_ref()).unwrap_or_default());
                    let value = attr
                        .unescape_value()
                        .map_or(Value::Null, |value| scalar_json(value.as_ref()));
                    attrs.insert(key, value);
                }
                let node = XmlNode {
                    tag,
                    attrs,
                    text: String::new(),
                    children: Vec::new(),
                };
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    return Ok(node);
                }
            }
            Ok(Event::Text(event)) => {
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(&String::from_utf8_lossy(event.as_ref()));
                }
            }
            Ok(Event::End(_)) => {
                let Some(node) = stack.pop() else {
                    return Err(ScatterError::Xml("unexpected closing tag".to_string()));
                };
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    return Ok(node);
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(ScatterError::Xml(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    Err(ScatterError::Xml("empty XML document".to_string()))
}

fn xml_children_dict(node: &XmlNode) -> Map<String, Value> {
    let mut out = Map::new();
    for child in &node.children {
        let value = xml_value(child);
        match out.get_mut(&child.tag) {
            Some(Value::Array(items)) => items.push(value),
            Some(existing) => {
                let old = std::mem::take(existing);
                *existing = Value::Array(vec![old, value]);
            }
            None => {
                out.insert(child.tag.clone(), value);
            }
        }
    }
    for (key, value) in &node.attrs {
        out.entry(key.clone()).or_insert_with(|| value.clone());
    }
    out
}

fn xml_value(node: &XmlNode) -> Value {
    if !node.children.is_empty() {
        let mut map = xml_children_dict(node);
        let text = node.text.trim();
        if !text.is_empty() {
            map.entry("#text".to_string())
                .or_insert_with(|| scalar_json(text));
        }
        return Value::Object(map);
    }

    for key in ["value", "name"] {
        if let Some(value) = node.attrs.get(key) {
            return value.clone();
        }
    }
    let text = node.text.trim();
    if !text.is_empty() {
        return scalar_json(text);
    }
    if !node.attrs.is_empty() {
        return Value::Object(node.attrs.clone());
    }
    Value::Null
}

fn normalize_partition(
    path: &Path,
    layout: &str,
    entry: Map<String, Value>,
) -> Result<ScatterPartition, ScatterError> {
    let name =
        normalize_none_string(get_first(&entry, &["partition_name", "name"])).ok_or_else(|| {
            ScatterError::Invalid(format!(
                "partition without partition_name in layout {layout}: {entry:?}"
            ))
        })?;
    let file_name = normalize_none_string(get_first(&entry, &["file_name", "filename"]));
    let known = [
        "partition_index",
        "partition_name",
        "file_name",
        "is_download",
        "type",
        "linear_start_addr",
        "physical_start_addr",
        "partition_size",
        "region",
        "storage",
        "boundary_check",
        "is_reserved",
        "operation_type",
        "is_upgradable",
        "empty_boot_needed",
        "combo_partsize_check",
        "reserve",
    ];
    let unknown_fields = entry
        .iter()
        .filter(|(key, _)| !known.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Ok(ScatterPartition {
        source: path.to_string_lossy().into_owned(),
        layout: layout.to_string(),
        index: normalize_none_string(get_first(&entry, &["partition_index"])),
        name,
        file_name,
        is_download: parse_bool(get_first(&entry, &["is_download"]), false),
        image_type: normalize_none_string(get_first(&entry, &["type"])),
        linear_start: parse_field_int(
            get_first(&entry, &["linear_start_addr"]),
            "linear_start_addr",
            0,
        )?,
        physical_start: parse_field_int(
            get_first(&entry, &["physical_start_addr"])
                .or_else(|| get_first(&entry, &["linear_start_addr"])),
            "physical_start_addr",
            0,
        )?,
        size: parse_field_int(get_first(&entry, &["partition_size"]), "partition_size", 0)?,
        region: value_to_string(get_first(&entry, &["region"]))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "UNKNOWN".to_string()),
        storage: normalize_none_string(get_first(&entry, &["storage"])),
        boundary_check: parse_bool(get_first(&entry, &["boundary_check"]), true),
        is_reserved: parse_bool(get_first(&entry, &["is_reserved"]), false),
        operation_type: normalize_none_string(get_first(&entry, &["operation_type"])),
        is_upgradable: entry
            .get("is_upgradable")
            .map(|value| parse_bool(Some(value), false)),
        empty_boot_needed: entry
            .get("empty_boot_needed")
            .map(|value| parse_bool(Some(value), false)),
        combo_partsize_check: entry
            .get("combo_partsize_check")
            .map(|value| parse_bool(Some(value), false)),
        raw: Value::Object(entry),
        unknown_fields,
    })
}

fn validate_layouts(
    layouts: &BTreeMap<String, Vec<ScatterPartition>>,
    warnings: &mut Vec<String>,
    errors: &mut Vec<String>,
) {
    for (layout, parts) in layouts {
        let mut seen: HashMap<(String, String), &ScatterPartition> = HashMap::new();
        for part in parts {
            if part.size < 0 {
                errors.push(format!("{layout}/{}: negative partition size", part.name));
            }
            if part.linear_start < 0 || part.physical_start < 0 {
                errors.push(format!("{layout}/{}: negative address", part.name));
            }
            let rf = part.region_family();
            let sf = part.storage_family();
            if matches!(layout.to_uppercase().as_str(), "UFS" | "EMMC")
                && matches!(rf.as_str(), "UFS" | "EMMC")
                && layout.to_uppercase() != rf
            {
                warnings.push(format!("{layout}/{}: layout is {layout} but region family is {rf}; preserving vendor data", part.name));
            }
            if matches!(layout.to_uppercase().as_str(), "UFS" | "EMMC")
                && matches!(sf.as_str(), "UFS" | "EMMC")
                && layout.to_uppercase() != sf
            {
                warnings.push(format!("{layout}/{}: layout is {layout} but storage family is {sf}; preserving vendor data", part.name));
            }
            let key = (part.region.clone(), part.name.to_lowercase());
            if let Some(old) = seen.get(&key) {
                let same_extent = old.linear_start == part.linear_start
                    && old.physical_start == part.physical_start
                    && old.size == part.size;
                let same_profile = old.file_name == part.file_name
                    && old.is_download == part.is_download
                    && old.operation_type == part.operation_type;
                if same_extent && same_profile {
                    warnings.push(format!(
                        "{layout}/{}/{}: exact duplicate declaration",
                        part.region, part.name
                    ));
                } else {
                    errors.push(format!(
                        "{layout}/{}/{}: ambiguous duplicate partition old={:#x}+{:#x} new={:#x}+{:#x}",
                        part.region, part.name, old.linear_start, old.size, part.linear_start, part.size
                    ));
                }
            } else {
                seen.insert(key, part);
            }
        }

        let mut by_region: BTreeMap<&str, Vec<&ScatterPartition>> = BTreeMap::new();
        for part in parts {
            if part.is_reserved || !part.boundary_check || part.size == 0 {
                continue;
            }
            by_region.entry(&part.region).or_default().push(part);
        }
        for (region, mut items) in by_region {
            items.sort_by_key(|part| (part.linear_start, part.end(), part.name.clone()));
            for pair in items.windows(2) {
                let prev = pair[0];
                let cur = pair[1];
                if prev.end() > cur.linear_start {
                    errors.push(format!(
                        "{layout}/{region}: overlap {} [{:#x},{:#x}) with {} [{:#x},{:#x})",
                        prev.name,
                        prev.linear_start,
                        prev.end(),
                        cur.name,
                        cur.linear_start,
                        cur.end()
                    ));
                }
            }
        }
    }
}

fn partition_to_json(
    part: &ScatterPartition,
    scatter_dir: Option<&Path>,
    firmware_dir: Option<&Path>,
    package_root: Option<&Path>,
    check_images: bool,
    image_search: bool,
) -> Value {
    let resolved = resolve_image_path(
        part.file_name.as_deref(),
        scatter_dir,
        firmware_dir,
        package_root,
        image_search,
    );
    let mut image_status = json!({
        "checked": check_images,
        "exists": resolved.exists,
        "size_bytes": null,
        "size_human": null,
        "fits_partition": null,
        "magic": null,
    });
    if check_images {
        if let Some(path) = resolved
            .resolved_path
            .as_deref()
            .filter(|_| resolved.exists == Some(true))
        {
            if let Ok(metadata) = fs::metadata(path) {
                let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
                image_status["size_bytes"] = json!(size);
                image_status["size_human"] = json!(human_size(size));
                image_status["fits_partition"] = json!(size <= part.size);
                image_status["magic"] = json!(image_magic(Path::new(path)));
            }
        }
    }
    let normalized_file_name = part.file_name.as_deref().map(normalize_path_display);
    let dirname = normalized_file_name
        .as_deref()
        .filter(|name| name.contains('/'))
        .and_then(|name| Path::new(name).parent())
        .map(|path| path.to_string_lossy().into_owned());
    json!({
        "source": part.source,
        "layout": part.layout,
        "index": part.index,
        "name": part.name,
        "file_name": part.file_name,
        "is_download": part.is_download,
        "type": part.image_type,
        "linear_start": part.linear_start,
        "physical_start": part.physical_start,
        "size": part.size,
        "end": part.end(),
        "region": part.region,
        "storage": part.storage,
        "operation_type": part.operation_type,
        "identity": {
            "partition_index": part.index,
            "partition_name": part.name,
            "base_name": part.base_name(),
            "canonical_name": part.canonical(),
            "slot": part.slot(),
            "is_a_b_slot": part.slot().is_some(),
            "role": part.role(),
            "traits": traits_for_partition(part),
        },
        "flash": {
            "is_download": part.is_download,
            "file_name": part.file_name,
            "image_type": part.image_type,
            "operation_type": part.operation_type,
            "boundary_check": part.boundary_check,
            "is_reserved": part.is_reserved,
            "is_upgradable": part.is_upgradable,
            "empty_boot_needed": part.empty_boot_needed,
            "combo_partsize_check": part.combo_partsize_check,
            "flashable_by_profile": part.flashable_by_profile(),
            "safe_default_flash": matches!(part.safety_class().as_str(), "boot_critical" | "firmware" | "android_system" | "regional"),
            "safety_class": part.safety_class(),
        },
        "address": {
            "linear_start": part.linear_start,
            "linear_start_hex": format!("{:#x}", part.linear_start),
            "physical_start": part.physical_start,
            "physical_start_hex": format!("{:#x}", part.physical_start),
            "size": part.size,
            "size_hex": format!("{:#x}", part.size),
            "size_human": human_size(part.size),
            "end": part.end(),
            "end_hex": format!("{:#x}", part.end()),
            "linear_equals_physical": part.linear_start == part.physical_start,
        },
        "storage_info": {
            "layout": part.layout,
            "region": part.region,
            "region_family": part.region_family(),
            "storage": part.storage,
            "storage_family": part.storage_family(),
            "address_space_key": format!("{}:{}", part.layout, part.region),
        },
        "image": {
            "basename": normalized_file_name.as_ref().and_then(|name| Path::new(name).file_name()).map(|name| name.to_string_lossy().into_owned()),
            "dirname": dirname,
            "path": resolved,
            "status": image_status,
        },
        "raw": part.raw,
        "unknown_fields": part.unknown_fields,
    })
}

fn resolve_image_path(
    file_name: Option<&str>,
    scatter_dir: Option<&Path>,
    firmware_dir: Option<&Path>,
    package_root: Option<&Path>,
    image_search: bool,
) -> ResolvedPath {
    let Some(original) = file_name else {
        return ResolvedPath {
            original: None,
            normalized: None,
            resolved_path: None,
            resolved_via: None,
            exists: None,
            is_absolute_input: false,
            input_style: None,
            contains_parent_reference: false,
            outside_package_root: None,
            warning: None,
        };
    };
    let normalized = normalize_path_display(original);
    let contains_parent = mixed_path_parts(&normalized)
        .iter()
        .any(|part| part == "..");
    let absolute_input = is_windows_absolute(original) || normalized.starts_with('/');
    let input_style = if original.contains('\\') || is_windows_absolute(original) {
        "windows"
    } else {
        "posix"
    };
    let mut candidates: Vec<(&str, PathBuf)> = Vec::new();
    if normalized.starts_with('/') {
        candidates.push(("absolute", PathBuf::from(&normalized)));
    } else if is_windows_absolute(original) {
        candidates.push(("windows_absolute", PathBuf::from(original)));
        let stripped = mixed_parts_path(original);
        if let Some(firmware_dir) = firmware_dir {
            candidates.push((
                "firmware_dir_windows_stripped",
                firmware_dir.join(&stripped),
            ));
        }
        if let Some(scatter_dir) = scatter_dir {
            candidates.push((
                "scatter_relative_windows_stripped",
                scatter_dir.join(&stripped),
            ));
        }
    } else {
        let rel = mixed_parts_path(&normalized);
        if let Some(firmware_dir) = firmware_dir {
            candidates.push(("firmware_dir_relative", firmware_dir.join(&rel)));
        }
        if let Some(scatter_dir) = scatter_dir {
            candidates.push(("scatter_relative", scatter_dir.join(&rel)));
        }
    }

    let mut warning = None;
    for (via, candidate) in &candidates {
        let candidate = absolutize(candidate);
        let outside = package_root.map(|root| !is_within(&candidate, root));
        if outside == Some(true) {
            warning = Some(format!(
                "resolved image path is outside package_root: {}",
                candidate.display()
            ));
            continue;
        }
        if candidate.exists() {
            return resolved_path_result(ResolvedPathParts {
                original,
                normalized: &normalized,
                resolved_path: Some(candidate),
                resolved_via: Some(*via),
                exists: Some(true),
                is_absolute_input: absolute_input,
                input_style,
                contains_parent_reference: contains_parent,
                outside_package_root: outside,
                warning,
            });
        }
    }

    let first_allowed = candidates.iter().find_map(|(via, candidate)| {
        let candidate = absolutize(candidate);
        let outside = package_root.map(|root| !is_within(&candidate, root));
        (outside != Some(true)).then_some((*via, candidate, outside))
    });

    if image_search {
        let mut seen = BTreeSet::new();
        for root in [firmware_dir, scatter_dir].into_iter().flatten() {
            let root = absolutize(root);
            if !seen.insert(root.clone()) {
                continue;
            }
            let basename = Path::new(&normalized)
                .file_name()
                .unwrap_or_else(|| OsStr::new(&normalized));
            match unique_basename_search(&root, basename) {
                Ok(Some(found)) => {
                    let outside = package_root.map(|package_root| !is_within(&found, package_root));
                    if outside == Some(true) {
                        warning = Some(format!(
                            "image-search result outside package_root: {}",
                            found.display()
                        ));
                        continue;
                    }
                    return resolved_path_result(ResolvedPathParts {
                        original,
                        normalized: &normalized,
                        resolved_path: Some(found),
                        resolved_via: Some("image_search_unique_basename"),
                        exists: Some(true),
                        is_absolute_input: absolute_input,
                        input_style,
                        contains_parent_reference: contains_parent,
                        outside_package_root: outside,
                        warning,
                    });
                }
                Ok(None) => {}
                Err(err) => {
                    warning = Some(err);
                    break;
                }
            }
        }
    }

    if let Some((via, candidate, outside)) = first_allowed {
        return resolved_path_result(ResolvedPathParts {
            original,
            normalized: &normalized,
            resolved_path: Some(candidate),
            resolved_via: Some(via),
            exists: Some(false),
            is_absolute_input: absolute_input,
            input_style,
            contains_parent_reference: contains_parent,
            outside_package_root: outside,
            warning,
        });
    }
    resolved_path_result(ResolvedPathParts {
        original,
        normalized: &normalized,
        resolved_path: None,
        resolved_via: None,
        exists: Some(false),
        is_absolute_input: absolute_input,
        input_style,
        contains_parent_reference: contains_parent,
        outside_package_root: package_root.map(|_| true),
        warning: warning.or_else(|| Some("no allowed image path candidate".to_string())),
    })
}

fn resolved_path_result(parts: ResolvedPathParts<'_>) -> ResolvedPath {
    ResolvedPath {
        original: Some(parts.original.to_string()),
        normalized: Some(parts.normalized.to_string()),
        resolved_path: parts
            .resolved_path
            .map(|path| path.to_string_lossy().into_owned()),
        resolved_via: parts.resolved_via.map(ToString::to_string),
        exists: parts.exists,
        is_absolute_input: parts.is_absolute_input,
        input_style: Some(parts.input_style.to_string()),
        contains_parent_reference: parts.contains_parent_reference,
        outside_package_root: parts.outside_package_root,
        warning: parts.warning,
    }
}

fn unique_basename_search(root: &Path, basename: &OsStr) -> Result<Option<PathBuf>, String> {
    let mut stack = vec![root.to_path_buf()];
    let mut first_match: Option<PathBuf> = None;
    while let Some(path) = stack.pop() {
        let entries = fs::read_dir(&path)
            .map_err(|err| format!("image-search failed under {}: {err}", root.display()))?;
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                stack.push(entry_path);
            } else if entry_path.file_name() == Some(basename) {
                let entry_path = absolutize(&entry_path);
                if let Some(first) = &first_match {
                    return Err(format!(
                        "ambiguous image basename {:?}: {}, {}",
                        basename,
                        first.display(),
                        entry_path.display()
                    ));
                }
                first_match = Some(entry_path);
            }
        }
    }
    Ok(first_match)
}

fn image_magic(path: &Path) -> Option<Value> {
    let mut file = fs::File::open(path).ok()?;
    let mut head = vec![0; 8192];
    let read = file.read(&mut head).ok()?;
    head.truncate(read);
    if head.is_empty() {
        return Some(json!({"kind": "empty"}));
    }
    let kind = if head.starts_with(b"ANDROID!") {
        "android_boot_or_recovery_image"
    } else if head.starts_with(b"AVB0") {
        "android_vbmeta_image"
    } else if head.get(..4) == Some(b"\x3a\xff\x26\xed") {
        "android_sparse_image"
    } else if head.starts_with(b"ELF") || head.starts_with(b"\x7fELF") {
        "elf"
    } else if head.len() >= 0x43a && matches!(&head[0x438..0x43a], b"\x53\xef" | b"\xef\x53") {
        "possible_ext_filesystem"
    } else if head
        .get(..1024)
        .is_some_and(|bytes| bytes.windows(8).any(|window| window == b"EFI PART"))
    {
        "gpt_or_disk_image"
    } else {
        "raw_or_unknown"
    };
    Some(json!({"kind": kind}))
}

fn flash_action(
    action: &str,
    part: &ScatterPartition,
    image: Option<Value>,
    reason: &str,
    warnings: Vec<String>,
) -> FlashAction {
    FlashAction {
        action: action.to_string(),
        execution_kind: execution_kind_for_action(action, part),
        partition: part.name.clone(),
        base_name: part.base_name(),
        slot: part.slot(),
        layout: part.layout.clone(),
        region: part.region.clone(),
        start: part.linear_start,
        start_hex: format!("{:#x}", part.linear_start),
        size: part.size,
        size_hex: format!("{:#x}", part.size),
        size_human: human_size(part.size),
        image,
        image_type: part.image_type.clone(),
        safety_class: part.safety_class(),
        reason: reason.to_string(),
        warnings,
    }
}

fn execution_kind_for_action(action: &str, part: &ScatterPartition) -> FlashActionExecutionKind {
    match action {
        "flash" => FlashActionExecutionKind::Flash,
        "wipe" if matches!(part.canonical().as_str(), "userdata" | "metadata") => {
            FlashActionExecutionKind::FormatData
        }
        "wipe" => FlashActionExecutionKind::EraseIfPresent,
        _ => FlashActionExecutionKind::Flash,
    }
}

fn skipped_partition(part: &ScatterPartition, reason: &str) -> SkippedPartition {
    SkippedPartition {
        partition: part.name.clone(),
        layout: part.layout.clone(),
        region: part.region.clone(),
        reason: reason.to_string(),
        safety_class: part.safety_class(),
        file_name: part.file_name.clone(),
    }
}

fn mode_allows_partition(
    part: &ScatterPartition,
    image_source: &ScatterPartition,
    mode: Mode,
    include_preloader: bool,
) -> (bool, String) {
    let canonical = part.canonical();
    let safety = part.safety_class();
    let flashable = image_source.flashable_by_profile() && part.size > 0;
    if matches!(safety.as_str(), "identity_or_calibration" | "dangerous") {
        return (false, format!("blocked safety class: {safety}"));
    }
    if canonical == "preloader" && !include_preloader {
        return (false, "preloader requires --include-preloader".to_string());
    }
    match mode {
        Mode::DryRun => {
            if WIPE_CANONICAL.contains(&canonical.as_str()) {
                return (false, "normalized as wipe-only in dry run".to_string());
            }
            if flashable {
                (true, "scatter profile selected".to_string())
            } else {
                (
                    false,
                    "not selected by scatter profile or no image".to_string(),
                )
            }
        }
        Mode::Selective => {
            if flashable {
                (true, "selected by user".to_string())
            } else {
                (
                    false,
                    "selected but not flashable by scatter profile".to_string(),
                )
            }
        }
        Mode::DirtyFlash | Mode::CleanFlash => {
            if !flashable {
                return (
                    false,
                    "not selected by scatter profile or no image".to_string(),
                );
            }
            if BOOTLOADER_CANONICAL.contains(&canonical.as_str())
                || BOOT_CHAIN_CANONICAL.contains(&canonical.as_str())
                || MODEM_CANONICAL.contains(&canonical.as_str())
                || MCU_FW_CANONICAL.contains(&canonical.as_str())
                || ANDROID_CANONICAL.contains(&canonical.as_str())
                || REGIONAL_CANONICAL.contains(&canonical.as_str())
            {
                (true, format!("allowed by {}", mode.as_python()))
            } else {
                (
                    false,
                    format!("not included in {} policy", mode.as_python()),
                )
            }
        }
    }
}

fn inherited_image_source_for_slot_b<'a>(
    part: &'a ScatterPartition,
    parts_by_name: &BTreeMap<String, &'a ScatterPartition>,
    slot_policy: SlotPolicy,
) -> &'a ScatterPartition {
    if slot_policy != SlotPolicy::B
        || part.slot().as_deref() != Some("b")
        || part.flashable_by_profile()
    {
        return part;
    }

    let source_name = format!("{}_a", part.base_name());
    match parts_by_name.get(&source_name) {
        Some(source) if source.flashable_by_profile() => source,
        _ => part,
    }
}

fn inherited_action_reason(
    base_reason: String,
    part: &ScatterPartition,
    image_source: &ScatterPartition,
) -> String {
    if part.name.eq_ignore_ascii_case(&image_source.name) {
        return base_reason;
    }

    let Some(source_slot) = image_source.slot() else {
        return base_reason;
    };
    format!("{base_reason}; inherited from slot {source_slot} image")
}

fn expand_requested_names(
    requested: &[String],
    available: &BTreeSet<String>,
    slot_policy: SlotPolicy,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for raw in requested {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        let lname = name.to_lowercase();
        if available.contains(&lname) || split_base_slot(&lname).1.is_some() {
            out.insert(lname);
            continue;
        }
        match slot_policy {
            SlotPolicy::Both | SlotPolicy::Auto => {
                let mut added = false;
                for slot in ["a", "b"] {
                    let candidate = format!("{lname}_{slot}");
                    if available.contains(&candidate) {
                        out.insert(candidate);
                        added = true;
                    }
                }
                if !added {
                    out.insert(lname);
                }
            }
            SlotPolicy::A | SlotPolicy::B | SlotPolicy::Active | SlotPolicy::Inactive => {
                let slot = if slot_policy == SlotPolicy::B {
                    "b"
                } else {
                    "a"
                };
                let candidate = format!("{lname}_{slot}");
                out.insert(if available.contains(&candidate) {
                    candidate
                } else {
                    lname
                });
            }
            SlotPolicy::AllFromScatter => {
                let matched = available
                    .iter()
                    .filter(|available| {
                        *available == &lname || available.starts_with(&format!("{lname}_"))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if matched.is_empty() {
                    out.insert(lname);
                } else {
                    out.extend(matched);
                }
            }
        }
    }
    out
}

fn part_matches_group(part: &ScatterPartition, groups: &[String]) -> bool {
    let canonical = part.canonical();
    groups
        .iter()
        .any(|group| group_members(group).contains(&canonical.as_str()))
}

fn group_names() -> BTreeSet<&'static str> {
    [
        "boot",
        "bootloader",
        "avb",
        "modem",
        "mcu",
        "firmware",
        "android",
        "regional",
        "full-safe",
    ]
    .into_iter()
    .collect()
}

fn group_members(group: &str) -> BTreeSet<&'static str> {
    match group.trim().to_lowercase().as_str() {
        "boot" => BOOT_CHAIN_CANONICAL.iter().copied().collect(),
        "bootloader" => BOOTLOADER_CANONICAL.iter().copied().collect(),
        "avb" => ["vbmeta", "vbmeta_system", "vbmeta_vendor"]
            .into_iter()
            .collect(),
        "modem" => MODEM_CANONICAL.iter().copied().collect(),
        "mcu" => MCU_FW_CANONICAL.iter().copied().collect(),
        "firmware" => BOOTLOADER_CANONICAL
            .iter()
            .chain(BOOT_CHAIN_CANONICAL)
            .chain(MODEM_CANONICAL)
            .chain(MCU_FW_CANONICAL)
            .copied()
            .collect(),
        "android" => ANDROID_CANONICAL.iter().copied().collect(),
        "regional" => REGIONAL_CANONICAL.iter().copied().collect(),
        "full-safe" => BOOTLOADER_CANONICAL
            .iter()
            .chain(BOOT_CHAIN_CANONICAL)
            .chain(MODEM_CANONICAL)
            .chain(MCU_FW_CANONICAL)
            .chain(ANDROID_CANONICAL)
            .chain(REGIONAL_CANONICAL)
            .copied()
            .collect(),
        _ => BTreeSet::new(),
    }
}

fn layout_hash(parts: &[ScatterPartition], include_profile: bool) -> String {
    let mut rows = parts
        .iter()
        .map(|part| {
            let mut row = json!([
                part.layout,
                part.region,
                part.name,
                part.linear_start,
                part.physical_start,
                part.size
            ]);
            if include_profile {
                let Value::Array(items) = &mut row else {
                    return row;
                };
                items.push(json!(part.is_download));
                items.push(json!(part.file_name));
                items.push(json!(part.operation_type));
            }
            row
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row.to_string());
    let encoded = serde_json::to_vec(&rows).unwrap_or_default();
    format!("{:x}", Sha256::digest(encoded))
}

fn traits_for_partition(part: &ScatterPartition) -> Vec<String> {
    let mut traits = Vec::new();
    if part.slot().is_some() {
        traits.push("a_b_slot".to_string());
    }
    if part.is_download {
        traits.push("download_selected".to_string());
    }
    if part.file_name.is_some() {
        traits.push("has_image".to_string());
    }
    if part.boundary_check {
        traits.push("boundary_check".to_string());
    }
    if part.is_reserved {
        traits.push("reserved".to_string());
    }
    let role = part.role();
    if role != "unknown" {
        traits.push(role);
    }
    let rf = part.region_family().to_lowercase();
    if rf != "unknown" {
        traits.push(format!("region_{rf}"));
    }
    let sf = part.storage_family().to_lowercase();
    if sf != "unknown" {
        traits.push(format!("storage_{sf}"));
    }
    traits
}

fn split_base_slot(name: &str) -> (String, Option<String>) {
    let lower = name.to_lowercase();
    for slot in ["_a", "_b"] {
        if let Some(base) = lower.strip_suffix(slot) {
            if !base.is_empty() {
                return (
                    base.to_string(),
                    Some(slot.trim_start_matches('_').to_string()),
                );
            }
        }
    }
    (name.to_string(), None)
}

fn matches_numbered(value: &str, prefix: &str) -> bool {
    value.len() == prefix.len() + 1
        && value.starts_with(prefix)
        && matches!(value.as_bytes().last(), Some(b'1' | b'2'))
}

fn is_numbered_vbmeta(value: &str) -> bool {
    let Some(last) = value.as_bytes().last() else {
        return false;
    };
    if !matches!(last, b'1' | b'2') {
        return false;
    }
    matches!(
        &value[..value.len() - 1],
        "vbmeta" | "vbmeta_system" | "vbmeta_vendor"
    )
}

fn scalar_json(value: &str) -> Value {
    let s = value.trim();
    if s.is_empty() {
        return Value::String(String::new());
    }
    match s.to_lowercase().as_str() {
        "true" | "yes" => return Value::Bool(true),
        "false" | "no" => return Value::Bool(false),
        _ => {}
    }
    parse_int(s, "scalar").map_or_else(
        |_| Value::String(s.to_string()),
        |num| Value::Number(num.into()),
    )
}

fn parse_bool(value: Option<&Value>, default: bool) -> bool {
    match value {
        None | Some(Value::Null) => default,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_i64().unwrap_or_default() != 0,
        Some(value) => match value_to_string(Some(value))
            .unwrap_or_default()
            .trim()
            .to_lowercase()
            .as_str()
        {
            "true" | "1" | "yes" | "y" | "on" => true,
            "false" | "0" | "no" | "n" | "off" => false,
            _ => default,
        },
    }
}

fn parse_field_int(
    value: Option<&Value>,
    field_name: &str,
    default: i64,
) -> Result<i64, ScatterError> {
    match value {
        Some(Value::Number(number)) => number
            .as_i64()
            .ok_or_else(|| ScatterError::Invalid(format!("invalid {field_name}: {number}"))),
        Some(Value::Bool(value)) => Ok(i64::from(*value)),
        Some(value) => parse_int(value_to_string(Some(value)).unwrap_or_default(), field_name),
        None => Ok(default),
    }
}

fn normalize_none_string(value: Option<&Value>) -> Option<String> {
    let text = value_to_string(value)?
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();
    if text.is_empty() {
        return None;
    }
    let normalized = text.replace('\\', "/");
    let last = normalized
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim()
        .to_uppercase();
    if NONE_TOKENS.contains(&text.trim().to_uppercase().as_str())
        || NONE_TOKENS.contains(&last.as_str())
    {
        None
    } else {
        Some(text)
    }
}

fn chipset_label(platform: Option<&str>, project: Option<&str>) -> Option<String> {
    normalize_chipset_value(platform).or_else(|| normalize_chipset_value(project))
}

fn normalize_chipset_value(value: Option<&str>) -> Option<String> {
    let text = value?.trim().trim_matches('"').trim_matches('\'').trim();
    if text.is_empty() {
        return None;
    }

    let stripped = text.strip_prefix('@').unwrap_or(text).trim();
    if stripped.is_empty() {
        return None;
    }

    let upper = stripped.to_uppercase();
    if NONE_TOKENS.contains(&upper.as_str())
        || matches!(upper.as_str(), "TMP" | "TEMP" | "TEMPORARY" | "UNKNOWN")
    {
        None
    } else {
        Some(stripped.to_string())
    }
}

fn normalize_path_display(value: &str) -> String {
    value.replace('\\', "/")
}

fn value_to_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(if *value { "true" } else { "false" }.to_string()),
        Value::Number(value) => Some(value.to_string()),
        other => Some(other.to_string()),
    }
}

fn get_first<'a>(map: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| map.get(*key))
}

fn find_general_value(general: &Value, wanted: &str) -> Option<String> {
    let wanted = wanted.to_lowercase();
    if !general.is_object() {
        return None;
    }

    let mut stack = vec![general];
    while let Some(value) = stack.pop() {
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    if key.to_lowercase().trim_start_matches('@') == wanted
                        && !matches!(value, Value::Array(_) | Value::Object(_))
                    {
                        if let Some(value) = normalize_none_string(Some(value)) {
                            return Some(value);
                        }
                    }
                }
                for child in map.values().rev() {
                    if child.is_object() || child.is_array() {
                        stack.push(child);
                    }
                }
            }
            Value::Array(items) => {
                for child in items.iter().rev() {
                    if child.is_object() || child.is_array() {
                        stack.push(child);
                    }
                }
            }
            _ => {}
        }
    }

    None
}

fn region_family(region: &str) -> String {
    let region = region.to_uppercase();
    if region.starts_with("UFS") {
        "UFS"
    } else if region.starts_with("EMMC") || (region.contains("BOOT") && region.contains("EMMC")) {
        "EMMC"
    } else {
        "UNKNOWN"
    }
    .to_string()
}

fn storage_family(storage: Option<&str>, layout: Option<&str>, region: Option<&str>) -> String {
    let storage = storage.unwrap_or_default().to_uppercase();
    if storage.contains("UFS") {
        "UFS".to_string()
    } else if storage.contains("EMMC") || storage.contains("MMC") {
        "EMMC".to_string()
    } else if layout.is_some_and(|layout| matches!(layout.to_uppercase().as_str(), "UFS" | "EMMC"))
    {
        layout.unwrap_or_default().to_uppercase()
    } else {
        region.map_or_else(|| "UNKNOWN".to_string(), region_family)
    }
}

fn strip_ns(tag: &str) -> String {
    tag.rsplit('}').next().unwrap_or(tag).to_string()
}

fn is_windows_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
        && bytes[0].is_ascii_alphabetic()
}

fn mixed_path_parts(path_text: &str) -> Vec<String> {
    let mut value = path_text
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .replace('\\', "/");
    if value.len() >= 3 && value.as_bytes()[1] == b':' && value.as_bytes()[2] == b'/' {
        value = value[3..].to_string();
    }
    value
        .trim_start_matches('/')
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(ToString::to_string)
        .collect()
}

fn mixed_parts_path(path_text: &str) -> PathBuf {
    mixed_path_parts(path_text).into_iter().collect()
}

fn is_within(path: &Path, root: &Path) -> bool {
    let path = absolutize(path);
    let root = absolutize(root);
    path.starts_with(root)
}

fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize_components(path)
    } else {
        normalize_components(
            &std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path),
        )
    }
}

fn normalize_components(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Crate version string used for CLI compatibility.
pub fn version() -> &'static str {
    VERSION
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scalar_json_should_parse_known_scalar_shapes() {
        assert_eq!(scalar_json("true"), json!(true));
        assert_eq!(scalar_json("0x10"), json!(16));
        assert_eq!(scalar_json("plain"), json!("plain"));
    }

    #[test]
    fn find_general_value_should_prefer_current_object_before_descending() {
        let general = json!({
            "nested": {
                "@platform": "inner"
            },
            "@platform": "outer"
        });

        let value = find_general_value(&general, "platform");

        assert_eq!(value.as_deref(), Some("outer"));
    }

    #[test]
    fn find_general_value_should_find_nested_scalar_iteratively() {
        let general = json!({
            "level1": {
                "level2": {
                    "@project": "found"
                }
            }
        });

        let value = find_general_value(&general, "project");

        assert_eq!(value.as_deref(), Some("found"));
    }

    #[test]
    fn find_general_value_should_find_values_nested_inside_arrays() {
        let general = json!({
            "general": [
                {
                    "info": [
                        {
                            "platform": "MT6789",
                            "project": "tb8781p1_64"
                        }
                    ]
                }
            ]
        });

        assert_eq!(
            find_general_value(&general, "platform").as_deref(),
            Some("MT6789")
        );
        assert_eq!(
            find_general_value(&general, "project").as_deref(),
            Some("tb8781p1_64")
        );
    }

    #[test]
    fn scatter_chipset_should_skip_tmp_placeholders() {
        let scatter = ScatterFile {
            path: PathBuf::from("scatter.txt"),
            format: "yaml".to_string(),
            text_hash: String::new(),
            platform: Some("@tmp".to_string()),
            project: Some("tb8781p1_64".to_string()),
            general: json!({}),
            layouts: BTreeMap::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
        };

        assert_eq!(scatter.chipset().as_deref(), Some("tb8781p1_64"));
    }

    #[test]
    fn scatter_chipset_should_strip_leading_at_from_real_values() {
        let scatter = ScatterFile {
            path: PathBuf::from("scatter.txt"),
            format: "xml".to_string(),
            text_hash: String::new(),
            platform: Some("@MT6789".to_string()),
            project: None,
            general: json!({}),
            layouts: BTreeMap::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
        };

        assert_eq!(scatter.chipset().as_deref(), Some("MT6789"));
    }

    #[test]
    fn parse_xml_scatter_should_preserve_nested_platform_metadata() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<root>
  <general name="MTK_PLATFORM_CFG">
    <config_version name="V2.2.0">
      <platform>MT6833</platform>
      <project>p661n_h334</project>
    </config_version>
  </general>
  <storage_type name="EMMC">
    <partition_index name="SYS0">
      <partition_name>preloader</partition_name>
      <file_name>preloader.bin</file_name>
      <is_download>true</is_download>
      <type>SV5_BL_BIN</type>
      <linear_start_addr>0x0</linear_start_addr>
      <physical_start_addr>0x0</physical_start_addr>
      <partition_size>0x100000</partition_size>
      <region>EMMC_BOOT1_BOOT2</region>
      <storage>HW_STORAGE_EMMC</storage>
      <boundary_check>true</boundary_check>
      <is_reserved>false</is_reserved>
      <operation_type>BOOTLOADERS</operation_type>
      <is_upgradable>true</is_upgradable>
      <empty_boot_needed>false</empty_boot_needed>
      <combo_partsize_check>false</combo_partsize_check>
      <reserve>0x00</reserve>
    </partition_index>
  </storage_type>
</root>"#;

        let parsed = parse_xml_scatter(xml).unwrap();

        assert_eq!(parsed.platform.as_deref(), Some("MT6833"));
        assert_eq!(parsed.project.as_deref(), Some("p661n_h334"));
    }

    #[test]
    fn unique_basename_search_should_error_when_multiple_matches_exist() {
        let temp = tempfile::tempdir().unwrap();
        let one = temp.path().join("a");
        let two = temp.path().join("b");
        fs::create_dir_all(&one).unwrap();
        fs::create_dir_all(&two).unwrap();
        fs::write(one.join("boot.img"), b"one").unwrap();
        fs::write(two.join("boot.img"), b"two").unwrap();

        let error = unique_basename_search(temp.path(), OsStr::new("boot.img")).unwrap_err();

        assert!(error.contains("ambiguous image basename"));
        assert!(error.contains("boot.img"));
    }
}
