//! Device info display helpers.

use std::collections::HashMap;

use terminal_output::chrome::{section_header, status_line, Tone};

/// Print a compact device info block from fastboot variables.
pub fn compact_device_info(vars: &HashMap<String, String>) -> String {
    let mut lines = Vec::new();
    lines.push(String::new());
    lines.push(section_header("DEVICE"));
    lines.push(String::new());

    let fields = [
        ("product", "product"),
        ("variant", "variant"),
        ("bootloader", "bootloader"),
        ("current-slot", "slot"),
        ("is-userspace", "mode"),
    ];

    for (var_key, label) in fields {
        if let Some(value) = vars.get(var_key) {
            lines.push(status_line(Tone::Info, label, value));
        }
    }

    if let Some(max) = vars.get("max-download-size") {
        lines.push(status_line(Tone::Info, "max-download", max));
    }

    lines.push(String::new());
    lines.join("\n")
}

/// Return a mock device info block for dry-run mode.
pub fn mock_device_info() -> String {
    let mut lines = Vec::new();
    lines.push(String::new());
    lines.push(section_header("DEVICE"));
    lines.push(String::new());
    lines.push(status_line(Tone::Accent, "mode", "dry-run (no device)"));
    lines.push(String::new());
    lines.join("\n")
}