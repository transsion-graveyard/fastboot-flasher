//! Table helpers — simple key-value line formatting.

use crossterm::style::Color;
use crossterm::style::Stylize;

/// Format a single key-value pair as a simple line.
pub fn kv_row(key: &str, value: &str) -> String {
    format!("{key}: {value}")
}

/// Format a single key-value pair with color styling.
pub fn kv_row_colored(key: &str, value: &str, key_color: Color, value_color: Color) -> String {
    format!("{}: {}", key.with(key_color), value.with(value_color))
}

/// Convert table data to simple key-value line format (no table, no borders).
/// Takes a slice of (key, value) pairs and formats them as "key: value" lines.
/// Accepts both string references and owned strings.
pub fn kv_table(pairs: &[(&str, impl AsRef<str>)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| kv_row(key, value.as_ref()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Convert table data to simple key-value line format with color styling.
/// Takes a slice of (key, value, key_color, value_color) tuples.
pub fn kv_table_colored(pairs: &[(&str, &str, Color, Color)]) -> String {
    pairs
        .iter()
        .map(|(key, value, key_color, value_color)| {
            kv_row_colored(key, value, *key_color, *value_color)
        })
        .collect::<Vec<_>>()
        .join("\n")
}
