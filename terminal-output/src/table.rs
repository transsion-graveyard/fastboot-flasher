//! Table helpers — compact table factory, styled header/label/value cells, and
//! horizontal centering.

use std::fmt;

use comfy_table::{
    modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL_CONDENSED, Attribute, Cell,
    ContentArrangement, Table,
};

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
