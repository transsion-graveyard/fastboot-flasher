use clap::Parser;
use force_fastboot::{run_force_fastboot, ForceFastbootOptions};

#[derive(Parser)]
#[command(about = "Force an MTK preloader device into fastboot mode")]
struct Args {
    #[arg(long, help = "Serial port to use instead of waiting for a new device")]
    port: Option<String>,

    #[arg(
        long,
        help = "Do not try to install a Linux udev rule when permission is denied"
    )]
    no_auto_udev: bool,
}

fn main() {
    let args = Args::parse();

    if let Err(e) = run(&args) {
        eprintln!("ERROR: {e:#}");
        std::process::exit(1);
    }
}

fn run(args: &Args) -> anyhow::Result<()> {
    run_force_fastboot(&ForceFastbootOptions {
        port: args.port.clone(),
        no_auto_udev: args.no_auto_udev,
    })
}
