use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn cli_should_report_checksum_xml_as_empty_full_json() {
    let mut cmd = Command::cargo_bin("mtk-scatter-parser").unwrap();

    cmd.args([
        "--full-json",
        "--storage",
        "all",
        "../tests/fixtures/checksum-scatter.xml",
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains("\"format\": \"checksum_xml\""))
    .stdout(predicate::str::contains("\"partition_count\": 0"));
}

#[test]
fn cli_should_exit_with_error_for_incomplete_both_slot_plan() {
    let mut cmd = Command::cargo_bin("mtk-scatter-parser").unwrap();

    cmd.args([
        "--json",
        "--mode",
        "selective",
        "--part",
        "boot",
        "--slot",
        "both",
        "../tmp/Global_scatter.txt",
    ])
    .assert()
    .failure()
    .stdout(predicate::str::contains("slot policy both requested"));
}
