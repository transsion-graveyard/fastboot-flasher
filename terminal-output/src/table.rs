//! Table helpers — compact table factory, styled header/label/value cells, and
//! horizontal centering.

use std::fmt;

use comfy_table::{
    modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL_CONDENSED, Attribute, Cell,
    ContentArrangement, Table,
};
use crossterm::style::Color as CrosstermColor;

use crate::chrome::center_block;

/// Re-export of [`comfy_table::Color`] for convenience.
pub use comfy_table::Color;

/// Create a compact, dynamically-arranged table with round corners.
pub fn compact_table() -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL_CONDENSED)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table
}

/// Create a cyan bold header cell.
pub fn header_cell(label: impl fmt::Display) -> Cell {
    Cell::new(label.to_string())
        .fg(Color::Cyan)
        .add_attribute(Attribute::Bold)
}

/// Create a cyan (non-bold) label cell.
pub fn label_cell(label: impl fmt::Display) -> Cell {
    Cell::new(label.to_string()).fg(Color::Cyan)
}

/// Create a plain value cell with no special styling.
pub fn value_cell(value: impl fmt::Display) -> Cell {
    Cell::new(value.to_string())
}

/// Create a value cell with the given foreground color.
pub fn colored_value_cell(value: impl fmt::Display, color: Color) -> Cell {
    Cell::new(value.to_string()).fg(color)
}

/// Render a table as a centered block within the terminal width.
pub fn centered_table(table: &Table) -> String {
    center_block(&table.to_string())
}

/// Format a single key-value pair as a simple line.
pub fn simple_kv_row(key: &str, value: &str) -> String {
    format!("{key}: {value}")
}

/// Format a single key-value pair with color styling.
pub fn simple_kv_row_colored(key: &str, value: &str, key_color: CrosstermColor, value_color: CrosstermColor) -> String {
    use crossterm::style::Stylize;
    format!("{}: {}", key.with(key_color), value.with(value_color))
}

/// Convert table data to simple key-value line format (no table, no borders).
/// Takes a slice of (key, value) pairs and formats them as "key: value" lines.
/// Accepts both string references and owned strings.
pub fn simple_kv_table(pairs: &[(&str, impl AsRef<str>)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| simple_kv_row(key, value.as_ref()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Convert table data to simple key-value line format with color styling.
/// Takes a slice of (key, value, key_color, value_color) tuples.
pub fn simple_kv_table_colored(pairs: &[(&str, &str, CrosstermColor, CrosstermColor)]) -> String {
    pairs
        .iter()
        .map(|(key, value, key_color, value_color)| {
            simple_kv_row_colored(key, value, *key_color, *value_color)
        })
        .collect::<Vec<_>>()
        .join("\n")
}
