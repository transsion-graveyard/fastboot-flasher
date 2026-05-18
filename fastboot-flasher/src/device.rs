use std::collections::HashMap;

use terminal_output::chrome::banner;
use terminal_output::table::{
    centered_table, colored_value_cell, compact_table, header_cell, label_cell, value_cell, Color,
};

const SUMMARY_KEYS: &[(&str, &str)] = &[
    ("serialno", "serial"),
    ("product", "product"),
    ("current-slot", "current slot"),
    ("unlocked", "unlocked"),
    ("secure", "secure"),
    ("is-userspace", "userspace fastboot"),
    ("slot-count", "slot count"),
    ("max-download-size", "max download"),
    ("version-bootloader", "bootloader"),
    ("version-baseband", "baseband"),
];

pub fn compact_device_info(vars: &HashMap<String, String>) -> String {
    let mut table = compact_table();
    table.set_header(vec![header_cell("Field"), header_cell("Value")]);
    for (key, label) in SUMMARY_KEYS {
        if let Some(value) = vars.get(*key).filter(|value| !value.trim().is_empty()) {
            table.add_row(vec![
                label_cell(*label),
                colored_value_cell(value.trim(), value_color(label, value.trim())),
            ]);
        }
    }
    for slot in ["a", "b"] {
        let retry = vars.get(&format!("slot-retry-count:{slot}"));
        let successful = vars.get(&format!("slot-successful:{slot}"));
        let unbootable = vars.get(&format!("slot-unbootable:{slot}"));
        if retry.is_some() || successful.is_some() || unbootable.is_some() {
            table.add_row(vec![
                label_cell(format!("slot {slot}")),
                value_cell(format!(
                    "retry={} successful={} unbootable={}",
                    retry.map(String::as_str).unwrap_or("?"),
                    successful.map(String::as_str).unwrap_or("?"),
                    unbootable.map(String::as_str).unwrap_or("?")
                )),
            ]);
        }
    }
    format!(
        "\n{}\n\n{}\n",
        banner("FASTBOOT DEVICE"),
        centered_table(&table)
    )
}

fn value_color(label: &str, value: &str) -> Color {
    let value = value.to_ascii_lowercase();
    match label {
        "unlocked" if value == "yes" => Color::Green,
        "unlocked" => Color::Yellow,
        "secure" if value == "no" => Color::Green,
        "secure" => Color::Red,
        "userspace fastboot" if value == "no" => Color::Green,
        "slot" if value.contains("unbootable=yes") => Color::Red,
        "slot" if value.contains("successful=yes") => Color::Green,
        "current slot" | "product" | "serial" => Color::Green,
        "max download" => Color::Cyan,
        _ => Color::Grey,
    }
}

pub fn mock_device_info() -> String {
    compact_device_info(&HashMap::from([
        ("serialno".to_string(), "mocked".to_string()),
        ("product".to_string(), "tb8781p1_64".to_string()),
        ("current-slot".to_string(), "a".to_string()),
        ("unlocked".to_string(), "yes".to_string()),
        ("secure".to_string(), "no".to_string()),
        ("is-userspace".to_string(), "no".to_string()),
        ("slot-count".to_string(), "2".to_string()),
        ("max-download-size".to_string(), "0x4000000".to_string()),
        ("version-bootloader".to_string(), "mocked".to_string()),
        ("version-baseband".to_string(), "mocked".to_string()),
        ("slot-retry-count:a".to_string(), "1".to_string()),
        ("slot-successful:a".to_string(), "yes".to_string()),
        ("slot-unbootable:a".to_string(), "no".to_string()),
        ("slot-retry-count:b".to_string(), "7".to_string()),
        ("slot-successful:b".to_string(), "no".to_string()),
        ("slot-unbootable:b".to_string(), "no".to_string()),
    ]))
}
