use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use mtk_scatter_parser::{
    build_flash_plan, parse_scatter, version, FlashPlanOptions, Mode, SlotPolicy, StorageSelect,
};
use serde_json::json;
use terminal_output::chrome::{banner, notice_box, section_header, Tone};
use terminal_output::table::{centered_table, compact_table, header_cell, label_cell, value_cell};

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
                eprintln!("{}", notice_box(Tone::Error, "fatal", &err.to_string()));
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
    println!("{}", banner("MTK SCATTER SUMMARY"));
    println!();
    println!("{}", section_header("Scatter Report"));
    println!();
    let mut scatter_table = compact_table();
    scatter_table.set_header(vec![header_cell("Field"), header_cell("Value")]);
    scatter_table.add_row(vec![label_cell("Status"), value_cell(status)]);
    scatter_table.add_row(vec![
        label_cell("Scatter"),
        value_cell(scatter.path.display()),
    ]);
    scatter_table.add_row(vec![label_cell("Format"), value_cell(&scatter.format)]);
    scatter_table.add_row(vec![
        label_cell("Platform"),
        value_cell(scatter.platform.as_deref().unwrap_or("?")),
    ]);
    scatter_table.add_row(vec![
        label_cell("Project"),
        value_cell(scatter.project.as_deref().unwrap_or("?")),
    ]);
    scatter_table.add_row(vec![
        label_cell("Layouts"),
        value_cell(if layout_desc.is_empty() {
            "none"
        } else {
            &layout_desc
        }),
    ]);
    scatter_table.add_row(vec![
        label_cell("Warnings"),
        value_cell(scatter.warnings.len()),
    ]);
    scatter_table.add_row(vec![label_cell("Errors"), value_cell(scatter.errors.len())]);
    println!("{}", centered_table(&scatter_table));
    println!();
    println!("{}", section_header("Plan Report"));
    println!();
    let mut plan_table = compact_table();
    plan_table.set_header(vec![header_cell("Field"), header_cell("Value")]);
    plan_table.add_row(vec![label_cell("Mode"), value_cell(&plan.mode)]);
    plan_table.add_row(vec![
        label_cell("Storage"),
        value_cell(&plan.storage_selection),
    ]);
    plan_table.add_row(vec![
        label_cell("Slot"),
        value_cell(&plan.slot_policy_effective),
    ]);
    plan_table.add_row(vec![
        label_cell("Flash"),
        value_cell(plan.summary.flash_count),
    ]);
    plan_table.add_row(vec![
        label_cell("Wipe"),
        value_cell(plan.summary.wipe_count),
    ]);
    plan_table.add_row(vec![
        label_cell("Skipped"),
        value_cell(plan.summary.skipped_count),
    ]);
    plan_table.add_row(vec![
        label_cell("Missing"),
        value_cell(plan.summary.missing_image_count),
    ]);
    plan_table.add_row(vec![
        label_cell("Oversized"),
        value_cell(plan.summary.oversized_image_count),
    ]);
    plan_table.add_row(vec![
        label_cell("Warnings"),
        value_cell(plan.summary.warning_count + plan.summary.action_warning_count),
    ]);
    plan_table.add_row(vec![
        label_cell("Errors"),
        value_cell(plan.summary.error_count),
    ]);
    println!("{}", centered_table(&plan_table));
    for warning in scatter.warnings.iter().take(20) {
        eprintln!(
            "{}",
            notice_box(Tone::Warning, "parser warning", warning.as_str())
        );
    }
    for error in scatter.errors.iter().take(20) {
        eprintln!(
            "{}",
            notice_box(Tone::Error, "parser error", error.as_str())
        );
    }
    for warning in plan.warnings.iter().take(20) {
        eprintln!(
            "{}",
            notice_box(Tone::Warning, "plan warning", warning.as_str())
        );
    }
    for error in plan.errors.iter().take(20) {
        eprintln!("{}", notice_box(Tone::Error, "plan error", error.as_str()));
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
