use clap::Parser;
use fastboot_flasher::{
    cli::{flash_modifier_without_flash, validate_args, Args, Command, FlashMode, SlotArg},
    device::{compact_device_info, mock_device_info},
    manual::{
        disable_vbmeta_actions, manual_flash_action, manual_flash_actions,
        standalone_disable_vbmeta_path,
    },
    plan::{build_plan_checked, mode_to_scatter, slot_to_scatter},
    progress::{
        action_summary, active_action_label, centered_prefix, compact_history_message,
        dry_run_steps, erase_history_message, fit_width, fixed_bar_width, flash_history_message,
        format_byte_pair, format_mm_ss, max_visible_width, progress_header, selective_option_label,
        should_confirm_before_simulation, skipped_erase_history_message,
        skipped_flash_history_message, visible_width, ActionSummary,
    },
    should_skip_failed_partition,
};
use fastboot_rs::{transport::nusb::NusbFastBootError, FastbootExecutionError};
use mtk_scatter_parser::{Mode, SlotPolicy};
use std::{collections::HashMap, path::PathBuf, time::Duration};

#[test]
fn bare_flash_defaults_to_dry_run_mode() {
    let args = Args::parse_from(["fastboot-flasher", "--flash", "scatter.xml"]);

    assert_eq!(args.flash_mode(), FlashMode::DryRun);
}

#[test]
fn firmware_upgrade_mode_maps_to_scatter_policy() {
    let args = Args::parse_from([
        "fastboot-flasher",
        "--flash",
        "scatter.xml",
        "--firmware-upgrade",
    ]);

    assert_eq!(mode_to_scatter(args.flash_mode()), Mode::FirmwareUpgrade);
}

#[test]
fn clean_flash_mode_maps_to_clean_flash_policy() {
    let args = Args::parse_from([
        "fastboot-flasher",
        "--flash",
        "scatter.xml",
        "--clean-flash",
    ]);

    assert_eq!(mode_to_scatter(args.flash_mode()), Mode::CleanFlash);
}

#[test]
fn clean_install_flag_should_no_longer_be_accepted() {
    let result = Args::try_parse_from([
        "fastboot-flasher",
        "--flash",
        "scatter.xml",
        "--clean-install",
    ]);

    assert!(result.is_err());
}

#[test]
fn include_preloader_flag_should_remain_supported() {
    let args = Args::parse_from([
        "fastboot-flasher",
        "--flash",
        "scatter.xml",
        "--dry-run",
        "--include-preloader",
    ]);

    assert!(args.include_preloader);
}

#[test]
fn flash_modifiers_without_flash_should_report_clear_error() {
    let args = Args::parse_from(["fastboot-flasher", "--dry-run"]);
    let error = flash_modifier_without_flash(&args).unwrap_err();

    assert!(error.contains("--flash <scatter>"));
    assert!(error.contains("--dry-run"));
    assert!(error.contains("fastboot-flasher disable-vbmeta"));
}

#[test]
fn firmware_upgrade_without_flash_should_be_rejected_by_clap() {
    let result = Args::try_parse_from(["fastboot-flasher", "--firmware-upgrade"]);

    assert!(result.is_err());
}

#[test]
fn clean_flash_without_flash_should_be_rejected_by_clap() {
    let result = Args::try_parse_from(["fastboot-flasher", "--clean-flash"]);

    assert!(result.is_err());
}

#[test]
fn selective_without_flash_should_be_rejected_by_clap() {
    let result = Args::try_parse_from(["fastboot-flasher", "--selective"]);

    assert!(result.is_err());
}

#[test]
fn slot_without_flash_should_be_rejected_by_clap() {
    let result = Args::try_parse_from(["fastboot-flasher", "--slot", "all"]);

    assert!(result.is_err());
}

#[test]
fn include_preloader_without_flash_should_be_rejected_by_clap() {
    let result = Args::try_parse_from(["fastboot-flasher", "--include-preloader"]);

    assert!(result.is_err());
}

#[test]
fn disable_vbmeta_subcommand_should_parse() {
    let args = Args::parse_from(["fastboot-flasher", "disable-vbmeta"]);

    assert!(matches!(args.command, Some(Command::DisableVbmeta)));
}

#[test]
fn gsi_subcommand_should_parse_image_path() {
    let args = Args::parse_from(["fastboot-flasher", "gsi", "system.img"]);

    assert!(matches!(
        args.command,
        Some(Command::Gsi { ref image }) if image == &PathBuf::from("system.img")
    ));
}

#[test]
fn scatter_subcommand_should_parse_clean_flash() {
    let args = Args::parse_from([
        "fastboot-flasher",
        "scatter",
        "scatter.xml",
        "--clean-flash",
    ]);

    assert!(matches!(
        args.command,
        Some(Command::Scatter {
            ref scatter,
            clean_flash: true,
            ..
        }) if scatter == &PathBuf::from("scatter.xml")
    ));
}

#[test]
fn manual_flash_subcommand_should_parse_partition_and_image() {
    let args = Args::parse_from(["fastboot-flasher", "flash", "boot", "boot.img"]);

    assert!(matches!(
        args.command,
        Some(Command::Flash {
            ref partition,
            ref image,
            ..
        })
            if partition == "boot" && image == &PathBuf::from("boot.img")
    ));
}

#[test]
fn manual_flash_subcommand_should_allow_nvram() {
    let args = Args::parse_from(["fastboot-flasher", "flash", "nvram", "nvram.bin"]);

    assert!(matches!(
        args.command,
        Some(Command::Flash {
            ref partition,
            ref image,
            ..
        })
            if partition == "nvram" && image == &PathBuf::from("nvram.bin")
    ));
}

#[test]
fn manual_flash_subcommand_should_parse_slot_b() {
    let args = Args::parse_from([
        "fastboot-flasher",
        "flash",
        "boot",
        "boot.img",
        "--slot",
        "b",
    ]);

    assert!(matches!(
        args.command,
        Some(Command::Flash {
            slot: Some(SlotArg::B),
            ..
        })
    ));
}

#[test]
fn reboot_subcommand_should_parse() {
    let args = Args::parse_from(["fastboot-flasher", "reboot"]);

    assert!(matches!(args.command, Some(Command::Reboot)));
}

#[test]
fn getvar_subcommand_should_parse() {
    let args = Args::parse_from(["fastboot-flasher", "getvar", "current-slot"]);

    assert!(matches!(
        args.command,
        Some(Command::Getvar { ref var }) if var == "current-slot"
    ));
}

#[test]
fn unlock_bootloader_subcommand_should_parse() {
    let args = Args::parse_from(["fastboot-flasher", "unlock-bootloader"]);

    assert!(matches!(args.command, Some(Command::UnlockBootloader)));
}

#[test]
fn lock_bootloader_subcommand_should_parse() {
    let args = Args::parse_from(["fastboot-flasher", "lock-bootloader"]);

    assert!(matches!(args.command, Some(Command::LockBootloader)));
}

#[test]
fn wipe_data_subcommand_should_parse_defaults() {
    let args = Args::parse_from(["fastboot-flasher", "wipe-data"]);

    assert!(matches!(
        args.command,
        Some(Command::WipeData {
            no_metadata: false,
            no_cache: false,
            erase_fallback: false,
        })
    ));
}

#[test]
fn wipe_data_subcommand_should_parse_optional_flags() {
    let args = Args::parse_from([
        "fastboot-flasher",
        "wipe-data",
        "--no-metadata",
        "--no-cache",
        "--erase-fallback",
    ]);

    assert!(matches!(
        args.command,
        Some(Command::WipeData {
            no_metadata: true,
            no_cache: true,
            erase_fallback: true,
        })
    ));
}

#[test]
fn format_userdata_subcommand_should_parse() {
    let args = Args::parse_from(["fastboot-flasher", "format", "userdata"]);

    assert!(matches!(
        args.command,
        Some(Command::Format {
            partition: ref target,
            erase_fallback: false,
        }) if target == "userdata"
    ));
}

#[test]
fn format_userdata_subcommand_should_allow_erase_fallback() {
    let args = Args::parse_from(["fastboot-flasher", "format", "userdata", "--erase-fallback"]);

    assert!(matches!(
        args.command,
        Some(Command::Format {
            partition: ref target,
            erase_fallback: true,
        }) if target == "userdata"
    ));
}

#[test]
fn global_dry_run_and_yes_should_work_after_subcommands() {
    let args = Args::parse_from([
        "fastboot-flasher",
        "flash",
        "boot",
        "boot.img",
        "--dry-run",
        "--yes",
    ]);

    assert!(args.dry_run);
    assert!(args.yes);
}

#[test]
fn manual_flash_action_should_accept_exact_partition_names() {
    let temp = tempfile::tempdir().unwrap();
    let image = temp.path().join("nvram.bin");
    std::fs::write(&image, [0u8; 16]).unwrap();

    let action = manual_flash_action("nvram", &image, None).unwrap();

    assert_eq!(action.partition, "nvram");
    assert_eq!(action.image, image);
    assert_eq!(action.size, 16);
    assert_eq!(action.reason, "manual image");
}

#[test]
fn manual_flash_action_should_apply_explicit_slot_suffix() {
    let temp = tempfile::tempdir().unwrap();
    let image = temp.path().join("boot.img");
    std::fs::write(&image, [0u8; 16]).unwrap();

    let action = manual_flash_action("boot", &image, Some(SlotArg::B)).unwrap();

    assert_eq!(action.partition, "boot_b");
}

#[test]
fn manual_flash_actions_should_expand_slot_all_to_both_slots() {
    let temp = tempfile::tempdir().unwrap();
    let image = temp.path().join("boot.img");
    std::fs::write(&image, [0u8; 16]).unwrap();

    let actions = manual_flash_actions("boot", &image, Some(SlotArg::All)).unwrap();

    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].partition, "boot_a");
    assert_eq!(actions[1].partition, "boot_b");
}

#[test]
fn manual_flash_action_should_reject_suffix_override() {
    let temp = tempfile::tempdir().unwrap();
    let image = temp.path().join("boot.img");
    std::fs::write(&image, [0u8; 16]).unwrap();

    let error = manual_flash_action("boot_b", &image, Some(SlotArg::A)).unwrap_err();

    assert!(error.to_string().contains("already has a slot suffix"));
}

#[test]
fn disable_vbmeta_actions_should_target_both_slots() {
    let temp = tempfile::tempdir().unwrap();
    let image = temp.path().join("empty_vbmeta.img");
    std::fs::write(&image, [0u8; 4]).unwrap();

    let actions = disable_vbmeta_actions(&image).unwrap();

    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].partition, "vbmeta_a");
    assert_eq!(actions[1].partition, "vbmeta_b");
    assert!(actions.iter().all(|action| action.image == image));
    assert!(actions
        .iter()
        .all(|action| action.reason == "disable-vbmeta empty image"));
}

#[test]
fn standalone_disable_vbmeta_path_should_materialize_absolute_file() {
    let path = standalone_disable_vbmeta_path().unwrap();

    assert!(path.is_absolute());
    assert!(path.is_file(), "expected file at {}", path.display());
}

#[test]
fn slot_all_means_both_ab_slots() {
    assert_eq!(slot_to_scatter(Some(SlotArg::All)), SlotPolicy::Both);
}

#[test]
fn mode_flags_are_mutually_exclusive() {
    let result = Args::try_parse_from([
        "fastboot-flasher",
        "--flash",
        "scatter.xml",
        "--dry-run",
        "--clean-flash",
    ]);

    assert!(result.is_err());
}

#[test]
fn dry_run_planning_does_not_require_image_files() {
    let temp = tempfile::tempdir().unwrap();
    let scatter = temp.path().join("minimal-scatter.xml");
    std::fs::write(
        &scatter,
        r#"<?xml version="1.0" encoding="utf-8"?>
<root>
  <general name="MTK_PLATFORM_CFG">
    <config_version name="V2.2.0">
      <platform>MT6789</platform>
      <project>demo_device</project>
    </config_version>
  </general>
  <storage_type name="EMMC">
    <partition_index name="SYS0">
      <partition_name>boot_a</partition_name>
      <file_name>boot.img</file_name>
      <is_download>true</is_download>
      <type>NORMAL_ROM</type>
      <linear_start_addr>0x0</linear_start_addr>
      <physical_start_addr>0x0</physical_start_addr>
      <partition_size>0x1000</partition_size>
      <region>EMMC_USER</region>
      <storage>HW_STORAGE_EMMC</storage>
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
      <file_name>boot.img</file_name>
      <is_download>true</is_download>
      <type>NORMAL_ROM</type>
      <linear_start_addr>0x1000</linear_start_addr>
      <physical_start_addr>0x1000</physical_start_addr>
      <partition_size>0x1000</partition_size>
      <region>EMMC_USER</region>
      <storage>HW_STORAGE_EMMC</storage>
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

    let plan = build_plan_checked(
        &scatter,
        FlashMode::DryRun,
        Some(SlotArg::All),
        false,
        Vec::new(),
        false,
    )
    .unwrap();

    assert!(plan
        .errors
        .iter()
        .all(|error| !error.contains("missing images")));
}

#[test]
fn dry_run_planning_should_resolve_parent_relative_images_within_package() {
    let temp = tempfile::tempdir().unwrap();
    let scatter_dir = temp.path().join("Global");
    std::fs::create_dir_all(&scatter_dir).unwrap();
    let scatter = scatter_dir.join("Global_scatter.txt");
    let image = temp.path().join("vbmeta_system.img");
    std::fs::write(&image, [0u8; 16]).unwrap();
    std::fs::write(
        &scatter,
        r#"
- general: MTK_PLATFORM_CFG
  config_version: V2.0.0
  platform: MT6833
  project: demo
- storage_type: UFS
  partition_index: SYS11
  partition_name: vbmeta_system_a
  file_name: ..\vbmeta_system.img
  is_download: true
  type: NORMAL_ROM
  linear_start_addr: 0xaf08000
  physical_start_addr: 0xaf08000
  partition_size: 0x800000
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

    let plan = build_plan_checked(
        &scatter,
        FlashMode::DryRun,
        None,
        false,
        Vec::new(),
        false,
    )
    .unwrap();

    let vbmeta_system = plan
        .actions
        .iter()
        .find(|action| action.partition == "vbmeta_system_a")
        .unwrap();

    assert_eq!(
        vbmeta_system.image_resolved_path(),
        Some(image.to_string_lossy().as_ref())
    );
}

#[test]
fn dry_run_steps_should_use_byte_totals() {
    let steps = dry_run_steps(9 * 1024 * 1024 * 1024, 1024);

    assert_eq!(
        steps.iter().map(|step| step.bytes).sum::<u64>(),
        9 * 1024 * 1024 * 1024
    );
    assert!(steps.len() > 10);
}

#[test]
fn dry_run_steps_should_keep_tiny_partitions_visible() {
    let steps = dry_run_steps(1024 * 1024, 1024);

    assert!(steps.len() >= 12);
}

#[test]
fn fixed_bar_width_should_be_clamped_from_terminal_width() {
    assert_eq!(fixed_bar_width(80), 13);
    assert_eq!(fixed_bar_width(120), 15);
    assert_eq!(fixed_bar_width(220), 15);
    assert_eq!(fixed_bar_width(70), 10);
}

#[test]
fn fit_width_should_shrink_to_available_space() {
    assert_eq!(fit_width(8, 10, 28), 8);
    assert_eq!(fit_width(12, 10, 28), 12);
    assert_eq!(fit_width(40, 10, 28), 28);
}

#[test]
fn centered_prefix_should_pad_labels_by_visible_width() {
    let prefix = centered_prefix("1/20", 20, 40);

    assert_eq!(visible_width(&prefix), 14);
    assert_eq!(prefix, "          1/20");
}

#[test]
fn max_visible_width_should_pick_the_longest_progress_message() {
    let width = max_visible_width([
        "flash vbmeta_a",
        "| flash vbmeta_system_a",
        "skipped flash vendor_boot_a",
    ]);

    assert_eq!(width, visible_width("skipped flash vendor_boot_a"));
}

#[test]
fn format_mm_ss_should_use_minutes_and_seconds_only() {
    assert_eq!(format_mm_ss(Duration::from_secs(1)), "00:01");
    assert_eq!(format_mm_ss(Duration::from_secs(65)), "01:05");
    assert_eq!(format_mm_ss(Duration::from_secs(3660)), "61:00");
}

#[test]
fn dry_run_confirmation_should_be_skipped_only_by_yes_flag() {
    assert!(should_confirm_before_simulation(false));
    assert!(!should_confirm_before_simulation(true));
}

#[test]
fn dry_run_speed_mib_flag_should_be_rejected() {
    let result = Args::try_parse_from([
        "fastboot-flasher",
        "--flash",
        "scatter.xml",
        "--dry-run",
        "--dry-run-speed-mib",
        "1",
    ]);

    assert!(result.is_err());
}

#[test]
fn reboot_subcommand_should_conflict_with_legacy_reboot_flag() {
    let args = Args::parse_from(["fastboot-flasher", "--reboot", "reboot"]);

    assert!(validate_args(&args).is_err());
}

#[test]
fn getvar_subcommand_should_conflict_with_legacy_getvar_flag() {
    let args = Args::parse_from([
        "fastboot-flasher",
        "--getvar",
        "product",
        "getvar",
        "current-slot",
    ]);

    assert!(validate_args(&args).is_err());
}

#[test]
fn scatter_subcommand_should_conflict_with_legacy_flash_flag() {
    let args = Args::parse_from([
        "fastboot-flasher",
        "--flash",
        "legacy.xml",
        "scatter",
        "next.xml",
    ]);

    assert!(validate_args(&args).is_err());
}

#[test]
fn action_summary_should_count_flash_wipe_and_bytes() {
    let summary = action_summary([("flash", 1024), ("wipe", 2048), ("skip", 4096)]);

    assert_eq!(summary.flash_count, 1);
    assert_eq!(summary.wipe_count, 1);
    assert_eq!(summary.skipped_count, 1);
    assert_eq!(summary.action_count(), 3);
    assert_eq!(summary.total_bytes, 3 * 1024);
}

#[test]
fn action_summary_should_preserve_large_u64_sizes() {
    let summary = action_summary([("flash", u64::MAX - 1), ("wipe", 1), ("skip", 4096)]);

    assert_eq!(summary.flash_count, 1);
    assert_eq!(summary.wipe_count, 1);
    assert_eq!(summary.total_bytes, u64::MAX);
}

#[test]
fn selective_option_label_should_not_leak_dry_run_reason() {
    let label = selective_option_label("boot_a", "boot_critical", "64.00 MiB");

    assert_eq!(label, "boot_a [boot_critical] 64.00 MiB");
    assert!(!label.contains("dry-run"));
}

#[test]
fn progress_header_should_promote_mode_and_summary() {
    let header = progress_header(
        ActionSummary {
            flash_count: 20,
            wipe_count: 0,
            skipped_count: 0,
            total_bytes: 12_430_000_000,
        },
        true,
    );

    assert_eq!(header, "20 actions 11.58 GiB");
}

#[test]
fn active_action_label_should_call_out_current_step() {
    assert_eq!(active_action_label(12, 20), "13/20");
}

#[test]
fn compact_history_message_should_be_single_line_summary() {
    let message = compact_history_message(0, 20, "flash", "vbmeta_a", 8 * 1024 * 1024, 27);

    assert_eq!(message, " 1/20 vbmeta_a --- 8.00 MiB");
    assert!(!message.contains('['));
}

#[test]
fn flash_history_message_should_render_success_compactly() {
    let message = flash_history_message(0, 20, "boot_a", 64 * 1024 * 1024, 25);

    assert_eq!(message, " 1/20 boot_a -- 64.00 MiB");
}

#[test]
fn skipped_flash_history_message_should_render_explicitly() {
    let message = skipped_flash_history_message(0, 20, "boot_a", 64 * 1024 * 1024, 38);

    assert_eq!(message, " 1/20 skipped flash boot_a - 64.00 MiB");
}

#[test]
fn erase_history_message_should_render_without_size() {
    let message = erase_history_message(0, 20, "metadata");

    assert_eq!(message, " 1/20 erase metadata");
}

#[test]
fn skipped_erase_history_message_should_render_without_size() {
    let message = skipped_erase_history_message(0, 20, "metadata");

    assert_eq!(message, " 1/20 skipped erase metadata");
}

#[test]
fn flash_history_message_should_render_actual_large_sizes() {
    let rendered = flash_history_message(0, 20, "super", 9 * 1024 * 1024 * 1024, 25);

    assert_eq!(rendered, " 1/20 super ---- 9.00 GiB");
}

#[test]
fn flash_history_message_should_keep_at_least_one_separator_dash() {
    let rendered = flash_history_message(19, 20, "userdata", 3 * 1024 * 1024 * 1024, 25);

    assert_eq!(rendered, "20/20 userdata - 3.00 GiB");
}

#[test]
fn format_byte_pair_should_use_fixed_width() {
    let tiny = format_byte_pair(1024 * 1024, 1024 * 1024);
    let large = format_byte_pair(9 * 1024 * 1024 * 1024, 9 * 1024 * 1024 * 1024);

    assert_eq!(tiny.len(), large.len());
    assert_eq!(tiny.trim(), "1.00 MiB/1.00 MiB");
    assert_eq!(large.trim(), "9.00 GiB/9.00 GiB");
}

#[test]
fn compact_device_info_should_include_useful_fields_only() {
    let vars = HashMap::from([
        ("serialno".to_string(), "123".to_string()),
        ("product".to_string(), "tb8781p1_64".to_string()),
        ("current-slot".to_string(), "a".to_string()),
        ("unlocked".to_string(), "yes".to_string()),
        ("secure".to_string(), "no".to_string()),
        ("is-userspace".to_string(), "no".to_string()),
        ("slot-count".to_string(), "2".to_string()),
        ("max-download-size".to_string(), "0x4000000".to_string()),
        (
            "version-bootloader".to_string(),
            "tb8781p1_64_20250305".to_string(),
        ),
        ("partition-size:boot_a".to_string(), "4000000".to_string()),
    ]);
    let info = compact_device_info(&vars);

    assert!(info.contains("FASTBOOT DEVICE"));
    assert!(info.contains("Field"));
    assert!(info.contains("product"));
    assert!(info.contains("tb8781p1_64"));
    assert!(info.contains("current slot"));
    assert!(!info.contains("partition-size"));
}

#[test]
fn mock_device_info_should_be_marked_as_mocked() {
    let info = mock_device_info();

    assert!(info.contains("mocked"));
    assert!(info.contains("product"));
    assert!(info.contains("tb8781p1_64"));
}

#[test]
fn should_skip_failed_partition_should_match_fastboot_failures_only() {
    let err = FastbootExecutionError::Fastboot(fastboot_flasher::FastbootError::Nusb(
        NusbFastBootError::FastbootFailed("flashing is not allowed".to_string()),
    ));

    assert!(should_skip_failed_partition(&err));
}
