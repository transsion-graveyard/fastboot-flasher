use std::{
    fs::{self, File},
    io::Write,
    path::PathBuf,
};

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
    let temp = write_global_yaml_fixture(false);
    let scatter = parse_scatter(temp.path().join("global-scatter.txt")).unwrap();

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
    let temp = write_global_yaml_fixture(false);
    let scatter = parse_scatter(temp.path().join("global-scatter.txt")).unwrap();
    let plan = build_flash_plan(
        &scatter,
        FlashPlanOptions {
            mode: Mode::Selective,
            slot_policy: SlotPolicy::Both,
            parts: vec!["boot".to_string()],
            firmware_dir: Some(temp.path().to_path_buf()),
            ..FlashPlanOptions::default()
        },
    );

    assert_eq!(plan.summary.flash_count, 1);
    assert_eq!(plan.summary.error_count, 1);
    assert!(plan.errors[0].contains("slot policy both requested"));
}

#[test]
fn build_flash_plan_should_synthesize_non_download_b_slots_for_slot_all_modes() {
    let temp = write_minimal_xml_fixture(true);
    let scatter = parse_scatter(temp.path().join("minimal-scatter.xml")).unwrap();
    let plan = build_flash_plan(
        &scatter,
        FlashPlanOptions {
            mode: Mode::DryRun,
            slot_policy: SlotPolicy::Both,
            firmware_dir: Some(temp.path().to_path_buf()),
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
    let temp = write_minimal_xml_fixture(true);
    let scatter = parse_scatter(temp.path().join("minimal-scatter.xml")).unwrap();
    let plan = build_flash_plan(
        &scatter,
        FlashPlanOptions {
            mode: Mode::DryRun,
            slot_policy: SlotPolicy::B,
            firmware_dir: Some(temp.path().to_path_buf()),
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
        .is_some_and(|path| path.ends_with("boot.img")));
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
    let temp = write_global_yaml_fixture(true);
    let scatter = parse_scatter(temp.path().join("global-scatter.txt")).unwrap();
    let plan = build_flash_plan(
        &scatter,
        FlashPlanOptions {
            mode: Mode::Selective,
            slot_policy: SlotPolicy::A,
            parts: vec!["boot".to_string()],
            firmware_dir: Some(temp.path().to_path_buf()),
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

fn write_global_yaml_fixture(write_boot_image: bool) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    if write_boot_image {
        fs::write(temp.path().join("boot.img"), [0x42; 16]).unwrap();
    }
    fs::write(
        temp.path().join("global-scatter.txt"),
        r#"
- general: MTK_PLATFORM_CFG
  config_version: V2.0.0
  platform: MT6789
  project: demo
- storage_type: UFS
  partition_index: SYS0
  partition_name: boot_a
  file_name: boot.img
  is_download: true
  type: NORMAL_ROM
  linear_start_addr: 0x0
  physical_start_addr: 0x0
  partition_size: 0x1000
  region: UFS_LU2
  storage: HW_STORAGE_UFS
  boundary_check: true
  is_reserved: false
  operation_type: UPDATE
  is_upgradable: true
  empty_boot_needed: false
  combo_partsize_check: false
  reserve: 0x00
- storage_type: UFS
  partition_index: SYS1
  partition_name: boot_b
  file_name: NONE
  is_download: false
  type: NORMAL_ROM
  linear_start_addr: 0x1000
  physical_start_addr: 0x1000
  partition_size: 0x1000
  region: UFS_LU2
  storage: HW_STORAGE_UFS
  boundary_check: true
  is_reserved: false
  operation_type: UPDATE
  is_upgradable: true
  empty_boot_needed: false
  combo_partsize_check: false
  reserve: 0x00
"#,
    )
    .unwrap();
    temp
}

fn write_minimal_xml_fixture(write_boot_image: bool) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    if write_boot_image {
        fs::write(temp.path().join("boot.img"), [0x42; 16]).unwrap();
        fs::write(temp.path().join("recovery.img"), [0x24; 16]).unwrap();
        fs::write(temp.path().join("dtbo.img"), [0x18; 16]).unwrap();
        fs::write(temp.path().join("vbmeta.img"), [0x36; 16]).unwrap();
    }
    fs::write(
        temp.path().join("minimal-scatter.xml"),
        r#"<?xml version="1.0" encoding="utf-8"?>
<root>
  <general name="MTK_PLATFORM_CFG">
    <config_version name="V2.2.0">
      <platform>MT6789</platform>
      <project>demo_device</project>
    </config_version>
  </general>
  <storage_type name="UFS">
    <partition_index name="SYS0">
      <partition_name>boot_a</partition_name>
      <file_name>boot.img</file_name>
      <is_download>true</is_download>
      <type>NORMAL_ROM</type>
      <linear_start_addr>0x0</linear_start_addr>
      <physical_start_addr>0x0</physical_start_addr>
      <partition_size>0x1000</partition_size>
      <region>UFS_LU2</region>
      <storage>HW_STORAGE_UFS</storage>
      <boundary_check>true</boundary_check>
      <is_reserved>false</is_reserved>
      <operation_type>UPDATE</operation_type>
      <is_upgradable>true</is_upgradable>
      <empty_boot_needed>false</empty_boot_needed>
      <combo_partsize_check>false</combo_partsize_check>
      <reserve>0x00</reserve>
    </partition_index>
    <partition_index name="SYS1">
      <partition_name>boot_b</partition_name>
      <file_name>NONE</file_name>
      <is_download>false</is_download>
      <type>NORMAL_ROM</type>
      <linear_start_addr>0x1000</linear_start_addr>
      <physical_start_addr>0x1000</physical_start_addr>
      <partition_size>0x1000</partition_size>
      <region>UFS_LU2</region>
      <storage>HW_STORAGE_UFS</storage>
      <boundary_check>true</boundary_check>
      <is_reserved>false</is_reserved>
      <operation_type>UPDATE</operation_type>
      <is_upgradable>true</is_upgradable>
      <empty_boot_needed>false</empty_boot_needed>
      <combo_partsize_check>false</combo_partsize_check>
      <reserve>0x00</reserve>
    </partition_index>
    <partition_index name="SYS2">
      <partition_name>vbmeta_a</partition_name>
      <file_name>vbmeta.img</file_name>
      <is_download>true</is_download>
      <type>NORMAL_ROM</type>
      <linear_start_addr>0x2000</linear_start_addr>
      <physical_start_addr>0x2000</physical_start_addr>
      <partition_size>0x1000</partition_size>
      <region>UFS_LU2</region>
      <storage>HW_STORAGE_UFS</storage>
      <boundary_check>true</boundary_check>
      <is_reserved>false</is_reserved>
      <operation_type>UPDATE</operation_type>
      <is_upgradable>true</is_upgradable>
      <empty_boot_needed>false</empty_boot_needed>
      <combo_partsize_check>false</combo_partsize_check>
      <reserve>0x00</reserve>
    </partition_index>
    <partition_index name="SYS3">
      <partition_name>vbmeta_b</partition_name>
      <file_name>NONE</file_name>
      <is_download>false</is_download>
      <type>NORMAL_ROM</type>
      <linear_start_addr>0x3000</linear_start_addr>
      <physical_start_addr>0x3000</physical_start_addr>
      <partition_size>0x1000</partition_size>
      <region>UFS_LU2</region>
      <storage>HW_STORAGE_UFS</storage>
      <boundary_check>true</boundary_check>
      <is_reserved>false</is_reserved>
      <operation_type>UPDATE</operation_type>
      <is_upgradable>true</is_upgradable>
      <empty_boot_needed>false</empty_boot_needed>
      <combo_partsize_check>false</combo_partsize_check>
      <reserve>0x00</reserve>
    </partition_index>
    <partition_index name="SYS4">
      <partition_name>recovery</partition_name>
      <file_name>recovery.img</file_name>
      <is_download>true</is_download>
      <type>NORMAL_ROM</type>
      <linear_start_addr>0x4000</linear_start_addr>
      <physical_start_addr>0x4000</physical_start_addr>
      <partition_size>0x1000</partition_size>
      <region>UFS_LU2</region>
      <storage>HW_STORAGE_UFS</storage>
      <boundary_check>true</boundary_check>
      <is_reserved>false</is_reserved>
      <operation_type>UPDATE</operation_type>
      <is_upgradable>true</is_upgradable>
      <empty_boot_needed>false</empty_boot_needed>
      <combo_partsize_check>false</combo_partsize_check>
      <reserve>0x00</reserve>
    </partition_index>
    <partition_index name="SYS5">
      <partition_name>dtbo</partition_name>
      <file_name>dtbo.img</file_name>
      <is_download>true</is_download>
      <type>NORMAL_ROM</type>
      <linear_start_addr>0x5000</linear_start_addr>
      <physical_start_addr>0x5000</physical_start_addr>
      <partition_size>0x1000</partition_size>
      <region>UFS_LU2</region>
      <storage>HW_STORAGE_UFS</storage>
      <boundary_check>true</boundary_check>
      <is_reserved>false</is_reserved>
      <operation_type>UPDATE</operation_type>
      <is_upgradable>true</is_upgradable>
      <empty_boot_needed>false</empty_boot_needed>
      <combo_partsize_check>false</combo_partsize_check>
      <reserve>0x00</reserve>
    </partition_index>
  </storage_type>
</root>"#,
    )
    .unwrap();
    temp
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
