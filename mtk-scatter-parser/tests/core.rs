use std::{fs::File, io::Write, path::PathBuf};

use mtk_scatter_parser::{
    build_flash_plan, canonical_name, human_size, parse_int, parse_scatter, safety_class,
    FlashPlanOptions, Mode, ScatterFile, ScatterPartition, SlotPolicy, StorageSelect,
};
use serde_json::json;

#[test]
fn parse_int_should_accept_decimal_and_hex_variants() {
    assert_eq!(parse_int("42", "field").unwrap(), 42);
    assert_eq!(parse_int("0x2a", "field").unwrap(), 42);
    assert_eq!(parse_int("2Ah", "field").unwrap(), 42);
}

#[test]
fn canonical_name_should_collapse_known_numbered_partitions() {
    assert_eq!(canonical_name("tee1"), "tee");
    assert_eq!(canonical_name("lk2"), "lk");
    assert_eq!(canonical_name("vbmeta_system2"), "vbmeta_system");
}

#[test]
fn safety_class_should_block_identity_and_dangerous_partitions() {
    assert_eq!(safety_class("nvram"), "identity_or_calibration");
    assert_eq!(safety_class("pgpt"), "dangerous");
    assert_eq!(safety_class("boot_a"), "boot_critical");
    assert_eq!(safety_class("csci"), "regional");
    assert_eq!(safety_class("super"), "android_system");
    assert_eq!(safety_class("vendor_boot"), "boot_critical");
    assert_eq!(safety_class("md1img"), "firmware");
}

#[test]
fn human_size_should_match_python_formatting() {
    assert_eq!(human_size(512), "512 B");
    assert_eq!(human_size(1024), "1.00 KiB");
    assert_eq!(human_size(8 * 1024 * 1024), "8.00 MiB");
}

#[test]
fn parse_scatter_should_select_ufs_layout_by_default() {
    let scatter = parse_scatter(PathBuf::from("../tests/fixtures/global-scatter.txt")).unwrap();

    assert_eq!(scatter.format, "yaml");
    assert_eq!(
        scatter
            .selected_layouts(StorageSelect::Auto)
            .keys()
            .collect::<Vec<_>>(),
        vec!["UFS"]
    );
}

#[test]
fn build_flash_plan_should_error_when_both_slots_are_incomplete() {
    let scatter = parse_scatter(PathBuf::from("../tests/fixtures/global-scatter.txt")).unwrap();
    let plan = build_flash_plan(
        &scatter,
        FlashPlanOptions {
            mode: Mode::Selective,
            slot_policy: SlotPolicy::Both,
            parts: vec!["boot".to_string()],
            firmware_dir: Some(PathBuf::from("tests/fixtures")),
            ..FlashPlanOptions::default()
        },
    );

    assert_eq!(plan.summary.flash_count, 1);
    assert_eq!(plan.summary.error_count, 1);
    assert!(plan.errors[0].contains("slot policy both requested"));
}

#[test]
fn build_flash_plan_should_synthesize_non_download_b_slots_for_slot_all_modes() {
    let scatter = parse_scatter(PathBuf::from("../tests/fixtures/minimal-scatter.xml")).unwrap();
    let plan = build_flash_plan(
        &scatter,
        FlashPlanOptions {
            mode: Mode::DryRun,
            slot_policy: SlotPolicy::Both,
            firmware_dir: Some(PathBuf::from("tests/fixtures")),
            ..FlashPlanOptions::default()
        },
    );

    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    assert!(plan
        .actions
        .iter()
        .any(|action| action.partition == "boot_b"));
    let boot_b = plan
        .actions
        .iter()
        .find(|action| action.partition == "boot_b")
        .unwrap();
    assert!(boot_b.reason.contains("inferred from slot a image"));
}

#[test]
fn build_flash_plan_should_inherit_slot_a_image_for_slot_b_mode() {
    let scatter = parse_scatter(PathBuf::from("../tests/fixtures/minimal-scatter.xml")).unwrap();
    let plan = build_flash_plan(
        &scatter,
        FlashPlanOptions {
            mode: Mode::DryRun,
            slot_policy: SlotPolicy::B,
            firmware_dir: Some(PathBuf::from("tests/fixtures")),
            ..FlashPlanOptions::default()
        },
    );

    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    let boot_b = plan
        .actions
        .iter()
        .find(|action| action.partition == "boot_b")
        .unwrap();
    assert_eq!(plan.summary.flash_count, 4);
    assert!(boot_b.reason.contains("inherited from slot a image"));
    assert!(boot_b
        .image_resolved_path()
        .is_some_and(|path| path.ends_with("tests/fixtures/boot.img")));
}

#[test]
fn synthesized_slot_action_should_recheck_image_against_target_partition_size() {
    let temp = tempfile::tempdir().unwrap();
    let image_path = temp.path().join("boot.img");
    let mut image = File::create(&image_path).unwrap();
    image.write_all(&vec![0x42; 2048]).unwrap();
    let scatter = synthetic_ab_scatter(temp.path().join("scatter.xml"));

    let plan = build_flash_plan(
        &scatter,
        FlashPlanOptions {
            mode: Mode::DryRun,
            slot_policy: SlotPolicy::Both,
            firmware_dir: Some(temp.path().to_path_buf()),
            package_root: Some(temp.path().to_path_buf()),
            check_images: true,
            ..FlashPlanOptions::default()
        },
    );

    let boot_b = plan
        .actions
        .iter()
        .find(|action| action.partition == "boot_b")
        .unwrap();
    assert_eq!(
        boot_b
            .image
            .as_ref()
            .and_then(|image| image.pointer("/status/fits_partition")),
        Some(&json!(false))
    );
    assert!(boot_b
        .warnings
        .iter()
        .any(|warning| warning.contains("image is larger than partition")));
}

#[test]
fn flash_action_should_expose_typed_image_path_fields() {
    let scatter = parse_scatter(PathBuf::from("../tests/fixtures/global-scatter.txt")).unwrap();
    let plan = build_flash_plan(
        &scatter,
        FlashPlanOptions {
            mode: Mode::Selective,
            slot_policy: SlotPolicy::A,
            parts: vec!["boot".to_string()],
            firmware_dir: Some(PathBuf::from("../tmp")),
            ..FlashPlanOptions::default()
        },
    );

    let action = plan
        .actions
        .iter()
        .find(|action| action.action == "flash")
        .unwrap();
    assert!(action.image_resolved_path().is_some());
    assert!(action.image_exists().is_some());
}

fn synthetic_ab_scatter(path: PathBuf) -> ScatterFile {
    ScatterFile {
        path,
        format: "test".to_string(),
        text_hash: String::new(),
        platform: None,
        project: None,
        general: json!({}),
        layouts: [(
            "UFS".to_string(),
            vec![
                synthetic_part("boot_a", Some("boot.img"), true, 4096),
                synthetic_part("boot_b", None, false, 1024),
            ],
        )]
        .into_iter()
        .collect(),
        warnings: Vec::new(),
        errors: Vec::new(),
    }
}

fn synthetic_part(
    name: &str,
    file_name: Option<&str>,
    is_download: bool,
    size: i64,
) -> ScatterPartition {
    ScatterPartition {
        source: "test".to_string(),
        layout: "UFS".to_string(),
        index: None,
        name: name.to_string(),
        file_name: file_name.map(ToString::to_string),
        is_download,
        image_type: None,
        linear_start: 0,
        physical_start: 0,
        size,
        region: "UFS_LU2".to_string(),
        storage: Some("UFS".to_string()),
        boundary_check: true,
        is_reserved: false,
        operation_type: None,
        is_upgradable: None,
        empty_boot_needed: None,
        combo_partsize_check: None,
        raw: json!({}),
        unknown_fields: Default::default(),
    }
}
