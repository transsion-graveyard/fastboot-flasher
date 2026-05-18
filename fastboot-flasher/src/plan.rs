use std::path::Path;

use mtk_scatter_parser::{FlashPlan, FlashPlanOptions, Mode, SlotPolicy, StorageSelect};

use crate::cli::{FlashMode, SlotArg};

pub fn mode_to_scatter(mode: FlashMode) -> Mode {
    match mode {
        FlashMode::DryRun => Mode::DryRun,
        FlashMode::FirmwareUpgrade => Mode::FirmwareUpgrade,
        FlashMode::CleanFlash => Mode::CleanFlash,
        FlashMode::Selective => Mode::Selective,
    }
}

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

pub fn build_plan(
    scatter_path: &Path,
    mode: FlashMode,
    slot: Option<SlotArg>,
    include_preloader: bool,
    parts: Vec<String>,
) -> anyhow::Result<FlashPlan> {
    build_plan_checked(scatter_path, mode, slot, include_preloader, parts, true)
}

pub fn build_plan_checked(
    scatter_path: &Path,
    mode: FlashMode,
    slot: Option<SlotArg>,
    include_preloader: bool,
    parts: Vec<String>,
    check_images: bool,
) -> anyhow::Result<FlashPlan> {
    let scatter = mtk_scatter_parser::parse_scatter(scatter_path)?;
    let firmware_dir = scatter_path.parent().map(Path::to_path_buf);
    Ok(mtk_scatter_parser::build_flash_plan(
        &scatter,
        FlashPlanOptions {
            mode: mode_to_scatter(mode),
            storage: StorageSelect::Auto,
            slot_policy: slot_to_scatter(slot),
            parts,
            groups: Vec::new(),
            firmware_dir,
            package_root: scatter_path.parent().map(Path::to_path_buf),
            check_images,
            image_search: false,
            include_preloader,
            allow_incomplete_slots: false,
        },
    ))
}
