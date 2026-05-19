#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    fastboot_flasher_gui_lib::init_logging();
    let args: Vec<String> = std::env::args().collect();
    if fastboot_flasher_gui_lib::is_gsi_worker_invocation(&args) {
        if let Err(error) = fastboot_flasher_gui_lib::run_gsi_worker_stdio() {
            tracing::error!(error, "gsi worker failed");
            std::process::exit(1);
        }
        return;
    }

    fastboot_flasher_gui_lib::run()
}
