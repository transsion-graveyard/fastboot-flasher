#![allow(missing_docs)]

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use mtk_scatter_parser::{
    build_flash_plan, parse_scatter, version, FlashPlanOptions, Mode, SlotPolicy, StorageSelect,
};
use serde_json::json;
use terminal_output::chrome::{simple_banner, simple_notice_box, simple_section_header, Tone};
use terminal_output::table::simple_kv_table;

#[derive(Debug, Parser)]
#[command(about = "Parse one MediaTek scatter file and generate a safe flash plan.")]
struct Args {
    /// Path to a single MTK scatter file.
    scatter: Option<PathBuf>,

    /// Print version and exit.
    #[arg(long)]
    version: bool,

    /// Emit compact JSON.
    #[arg(long)]
    json: bool,

    /// Include rich parsed partition JSON plus flash plan.
    #[arg(long)]
    full_json: bool,

    /// Storage layout selection; default auto prefers UFS.
    #[arg(long, value_enum, default_value_t = StorageSelect::Auto)]
    storage: StorageSelect,

    /// Flash plan mode.
    #[arg(long, value_enum, default_value_t = Mode::DryRun)]
    mode: Mode,

    /// Slot selection policy.
    #[arg(long, value_enum, default_value_t = SlotPolicy::Auto)]
    slot: SlotPolicy,

    /// Partition/base name for selective mode; repeatable.
    #[arg(long = "part")]
    parts: Vec<String>,

    /// Partition group for selective mode; repeatable.
    #[arg(long = "group")]
    groups: Vec<String>,

    /// Image root directory. Defaults to scatter file directory.
    #[arg(long)]
    firmware_dir: Option<PathBuf>,

    /// Trusted package root boundary for ../ paths.
    #[arg(long)]
    package_root: Option<PathBuf>,

    /// If exact scatter path is missing, recursively search for a unique basename.
    #[arg(long)]
    image_search: bool,

    /// Check image existence, size, fit, and basic magic.
    #[arg(long)]
    check_images: bool,

    /// Allow preloader in firmware-upgrade/clean-flash plans.
    #[arg(long)]
    include_preloader: bool,

    /// Warn instead of error if --slot both plans only one slot.
    #[arg(long)]
    allow_incomplete_slots: bool,

    /// Exit non-zero on parser or plan warnings as well as errors.
    #[arg(long)]
    strict: bool,
}

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("fatal: {err:#}");
            1
        }
    };
    std::process::exit(code);
}

fn run() -> anyhow::Result<i32> {
    let args = Args::parse();
    if args.version {
        println!("mtk-scatter-parser {}", version());
        return Ok(0);
    }

    let Some(scatter_path) = args.scatter else {
        eprintln!("error: scatter path required");
        return Ok(2);
    };
    let firmware_dir = args
        .firmware_dir
        .clone()
        .or_else(|| scatter_path.parent().map(PathBuf::from));

    let parsed = parse_scatter(&scatter_path);
    let (scatter, plan) = match parsed {
        Ok(scatter) => {
            let plan = build_flash_plan(
                &scatter,
                FlashPlanOptions {
                    mode: args.mode,
                    storage: args.storage,
                    slot_policy: args.slot,
                    parts: args.parts.clone(),
                    groups: args.groups.clone(),
                    firmware_dir: firmware_dir.clone(),
                    package_root: args.package_root.clone(),
                    check_images: args.check_images,
                    image_search: args.image_search,
                    include_preloader: args.include_preloader,
                    allow_incomplete_slots: args.allow_incomplete_slots,
                },
            );
            (scatter, plan)
        }
        Err(err) => {
            if args.json || args.full_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "tool": "mtk-scatter-parser",
                        "version": version(),
                        "fatal_error": err.to_string(),
                    }))
                    .context("serialize fatal JSON")?
                );
            } else {
                eprintln!("{}", simple_notice_box(Tone::Error, "fatal", &err.to_string()));
            }
            return Ok(1);
        }
    };

    if args.json || args.full_json {
        let out = if args.full_json {
            let mut value = scatter.to_json(
                args.storage,
                firmware_dir.as_deref(),
                args.package_root.as_deref(),
                args.check_images,
                args.image_search,
                args.storage == StorageSelect::All,
            );
            value["flash_plan"] = serde_json::to_value(&plan).context("serialize plan")?;
            value
        } else {
            json!({
                "tool": "mtk-scatter-parser",
                "version": version(),
                "source": scatter.path.to_string_lossy(),
                "format": scatter.format,
                "platform": scatter.platform,
                "project": scatter.project,
                "chipset": scatter.chipset(),
                "storage_selection": storage_name(args.storage),
                "selected_layouts": scatter.selected_layouts(args.storage).keys().collect::<Vec<_>>(),
                "parser_warnings": scatter.warnings,
                "parser_errors": scatter.errors,
                "flash_plan": plan,
            })
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&out).context("serialize JSON")?
        );
    } else {
        print_summary(&scatter, args.storage, &plan);
    }

    let hard_errors = !scatter.errors.is_empty() || !plan.errors.is_empty();
    let hard_warnings = args.strict && (!scatter.warnings.is_empty() || !plan.warnings.is_empty());
    Ok(i32::from(hard_errors || hard_warnings))
}

fn print_summary(
    scatter: &mtk_scatter_parser::ScatterFile,
    storage: StorageSelect,
    plan: &mtk_scatter_parser::FlashPlan,
) {
    println!();
    let selected = scatter.selected_layouts(storage);
    let layout_desc = selected
        .iter()
        .map(|(name, parts)| format!("{name}:{}", parts.len()))
        .collect::<Vec<_>>()
        .join(", ");
    let status = if scatter.errors.is_empty() {
        "OK"
    } else {
        "ERR"
    };
    println!("{}", simple_banner("MTK SCATTER SUMMARY"));
    println!();
    println!("{}", simple_section_header("Scatter Report"));
    println!();
    let scatter_pairs = vec![
        ("Status", status.to_string()),
        ("Scatter", scatter.path.display().to_string()),
        ("Format", scatter.format.clone()),
        ("Platform", scatter.platform.as_deref().unwrap_or("?").to_string()),
        ("Project", scatter.project.as_deref().unwrap_or("?").to_string()),
        ("Layouts", if layout_desc.is_empty() { "none".to_string() } else { layout_desc.clone() }),
        ("Warnings", scatter.warnings.len().to_string()),
        ("Errors", scatter.errors.len().to_string()),
    ];
    println!("{}", simple_kv_table(&scatter_pairs));
    println!();
    println!("{}", simple_section_header("Plan Report"));
    println!();
    let plan_pairs = vec![
        ("Mode", plan.mode.to_string()),
        ("Storage", plan.storage_selection.to_string()),
        ("Slot", plan.slot_policy_effective.to_string()),
        ("Flash", plan.summary.flash_count.to_string()),
        ("Wipe", plan.summary.wipe_count.to_string()),
        ("Skipped", plan.summary.skipped_count.to_string()),
        ("Missing", plan.summary.missing_image_count.to_string()),
        ("Oversized", plan.summary.oversized_image_count.to_string()),
        ("Warnings", (plan.summary.warning_count + plan.summary.action_warning_count).to_string()),
        ("Errors", plan.summary.error_count.to_string()),
    ];
    println!("{}", simple_kv_table(&plan_pairs));
    for warning in scatter.warnings.iter().take(20) {
        eprintln!(
            "{}",
            simple_notice_box(Tone::Warning, "parser warning", warning.as_str())
        );
    }
    for error in scatter.errors.iter().take(20) {
        eprintln!(
            "{}",
            simple_notice_box(Tone::Error, "parser error", error.as_str())
        );
    }
    for warning in plan.warnings.iter().take(20) {
        eprintln!(
            "{}",
            simple_notice_box(Tone::Warning, "plan warning", warning.as_str())
        );
    }
    for error in plan.errors.iter().take(20) {
        eprintln!("{}", simple_notice_box(Tone::Error, "plan error", error.as_str()));
    }
}

fn storage_name(storage: StorageSelect) -> &'static str {
    match storage {
        StorageSelect::Auto => "auto",
        StorageSelect::All => "all",
        StorageSelect::Ufs => "ufs",
        StorageSelect::Emmc => "emmc",
    }
}
