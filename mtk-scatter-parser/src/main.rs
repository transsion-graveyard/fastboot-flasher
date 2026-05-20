//! CLI for inspecting MediaTek scatter manifests and preview plans.

use std::path::PathBuf;

use clap::Parser;
use mtk_scatter_parser::{
    build_preview_plan, load_scatter_manifest, Mode, PreviewPlanOptions, SlotPolicy, StorageSelect,
};

#[derive(Debug, Parser)]
#[command(name = "mtk-scatter-parser")]
#[command(about = "Inspect MediaTek scatter manifests and preview flash plans")]
struct Cli {
    #[arg(long)]
    json: bool,
    #[arg(long = "full-json")]
    full_json: bool,
    #[arg(long, value_enum, default_value_t = StorageSelect::Auto)]
    storage: StorageSelect,
    #[arg(long, value_enum, default_value_t = Mode::DryRun)]
    mode: Mode,
    #[arg(long = "slot", value_enum, default_value_t = SlotPolicy::Auto)]
    slot_policy: SlotPolicy,
    #[arg(long = "part")]
    parts: Vec<String>,
    #[arg(long = "group")]
    groups: Vec<String>,
    #[arg(long)]
    firmware_dir: Option<PathBuf>,
    #[arg(long)]
    package_root: Option<PathBuf>,
    #[arg(long)]
    check_images: bool,
    #[arg(long)]
    image_search: bool,
    #[arg(long)]
    include_preloader: bool,
    #[arg(long)]
    allow_incomplete_slots: bool,
    scatter: PathBuf,
}

fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(cli) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let manifest =
        load_scatter_manifest(&cli.scatter).map_err(|error| format!("parse scatter: {error}"))?;
    let firmware_dir = cli
        .firmware_dir
        .or_else(|| cli.scatter.parent().map(PathBuf::from));
    let package_root = cli.package_root.or_else(|| {
        cli.scatter
            .parent()
            .and_then(|path| path.parent())
            .map(PathBuf::from)
            .or_else(|| firmware_dir.clone())
    });

    if cli.full_json {
        let value = manifest.to_json(
            cli.storage,
            firmware_dir.as_deref(),
            package_root.as_deref(),
            cli.check_images,
            cli.image_search,
            cli.storage == StorageSelect::All,
        );
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|error| format!("serialize manifest json: {error}"))?
        );
        return Ok(());
    }

    let plan = build_preview_plan(
        &manifest,
        PreviewPlanOptions {
            mode: cli.mode,
            storage: cli.storage,
            slot_policy: cli.slot_policy,
            parts: cli.parts,
            groups: cli.groups,
            firmware_dir,
            package_root,
            check_images: cli.check_images || cli.json,
            image_search: cli.image_search,
            include_preloader: cli.include_preloader,
            allow_incomplete_slots: cli.allow_incomplete_slots,
        },
    );

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&plan)
                .map_err(|error| format!("serialize plan json: {error}"))?
        );
    } else {
        println!(
            "mode={} storage={} slot-policy={} actions={}",
            plan.mode,
            plan.storage_selection,
            plan.slot_policy_effective,
            plan.actions.len()
        );
        for action in &plan.actions {
            println!(
                "{} {} {} [{}] - {}",
                action.action,
                action.partition,
                action.size_human,
                action.safety_class,
                action.reason
            );
        }
    }

    if plan.errors.is_empty() {
        Ok(())
    } else {
        Err(plan.errors.join("\n"))
    }
}
