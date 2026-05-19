//! Scatter-file plan building: convert CLI arguments into a [`FlashPlan`].

use std::path::Path;

use mtk_scatter_parser::{FlashPlan, FlashPlanOptions, Mode, SlotPolicy, StorageSelect};

use crate::cli::{FlashMode, SlotArg};

/// Convert a [`FlashMode`] to the [`mtk_scatter_parser::Mode`] equivalent.
pub fn mode_to_scatter(mode: FlashMode) -> Mode {
    match mode {
        FlashMode::DryRun => Mode::DryRun,
        FlashMode::FirmwareUpgrade => Mode::FirmwareUpgrade,
        FlashMode::CleanFlash => Mode::CleanFlash,
        FlashMode::Selective => Mode::Selective,
    }
}

/// Convert an optional [`SlotArg`] to a [`SlotPolicy`].
pub fn slot_to_scatter(slot: Option<SlotArg>) -> SlotPolicy {
    match slot {
        Some(SlotArg::A) => SlotPolicy::A,
        Some(SlotArg::B) => SlotPolicy::B,
        Some(SlotArg::Active) => SlotPolicy::Active,
        Some(SlotArg::Inactive) => SlotPolicy::Inactive,
        Some(SlotArg::All) => SlotPolicy::Both,
        None => SlotPolicy::Auto,
    }
}

/// Build a [`FlashPlan`] from a scatter file, with image existence checks
/// enabled.
pub fn build_plan(
    scatter_path: &Path,
    mode: FlashMode,
    slot: Option<SlotArg>,
    include_preloader: bool,
    parts: &[String],
) -> anyhow::Result<FlashPlan> {
    build_plan_checked(scatter_path, mode, slot, include_preloader, parts, true)
}

/// Build a [`FlashPlan`] from a scatter file, optionally checking that images
/// exist on disk.
pub fn build_plan_checked(
    scatter_path: &Path,
    mode: FlashMode,
    slot: Option<SlotArg>,
    include_preloader: bool,
    parts: &[String],
    check_images: bool,
) -> anyhow::Result<FlashPlan> {
    let scatter = mtk_scatter_parser::parse_scatter(scatter_path)?;
    let firmware_dir = scatter_path.parent().map(Path::to_path_buf);
    let package_root = scatter_path
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| firmware_dir.clone());
    Ok(mtk_scatter_parser::build_flash_plan(
        &scatter,
        FlashPlanOptions {
            mode: mode_to_scatter(mode),
            storage: StorageSelect::Auto,
            slot_policy: slot_to_scatter(slot),
            parts: parts.to_vec(),
            groups: Vec::new(),
            firmware_dir,
            package_root,
            check_images,
            image_search: false,
            include_preloader,
            allow_incomplete_slots: false,
        },
    ))
}
