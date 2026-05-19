//! CLI integration tests for `mtk-scatter-parser`.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;

#[test]
fn cli_should_report_checksum_xml_as_empty_full_json() {
    let temp = tempfile::tempdir().unwrap();
    let scatter = temp.path().join("checksum-scatter.xml");
    fs::write(
        &scatter,
        r#"<?xml version="1.0" encoding="utf-8"?>
<scatter_checksum>
  <checksum>deadbeef</checksum>
</scatter_checksum>
"#,
    )
    .unwrap();

    let mut cmd = Command::cargo_bin("mtk-scatter-parser").unwrap();

    cmd.args(["--full-json", "--storage", "all"])
        .arg(&scatter)
        .assert()
        .success()
        .stdout(predicate::str::contains("\"format\": \"checksum_xml\""))
        .stdout(predicate::str::contains("\"partition_count\": 0"));
}

#[test]
fn cli_should_exit_with_error_for_incomplete_both_slot_plan() {
    let temp = tempfile::tempdir().unwrap();
    let scatter = temp.path().join("Global_scatter.txt");
    fs::write(
        &scatter,
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

    let mut cmd = Command::cargo_bin("mtk-scatter-parser").unwrap();

    cmd.args([
        "--json",
        "--mode",
        "selective",
        "--part",
        "boot",
        "--slot",
        "both",
    ])
    .arg(&scatter)
    .assert()
    .failure()
    .stdout(predicate::str::contains("slot policy both requested"));
}
