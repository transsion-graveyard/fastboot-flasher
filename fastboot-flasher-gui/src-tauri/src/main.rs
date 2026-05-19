#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if fastboot_flasher_gui_lib::is_gsi_worker_invocation(&args) {
        if let Err(error) = fastboot_flasher_gui_lib::run_gsi_worker_stdio() {
            eprintln!("[gsi-worker] {error}");
            std::process::exit(1);
        }
        return;
    }

    fastboot_flasher_gui_lib::run()
}
