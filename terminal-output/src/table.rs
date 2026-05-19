use std::fmt;

use comfy_table::{
    modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL_CONDENSED, Attribute, Cell,
    ContentArrangement, Table,
};

use crate::chrome::center_block;

pub use comfy_table::Color;

pub fn compact_table() -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL_CONDENSED)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table
}

pub fn header_cell(label: impl fmt::Display) -> Cell {
    Cell::new(label.to_string())
        .fg(Color::Cyan)
        .add_attribute(Attribute::Bold)
}

pub fn label_cell(label: impl fmt::Display) -> Cell {
    Cell::new(label.to_string()).fg(Color::Cyan)
}

pub fn value_cell(value: impl fmt::Display) -> Cell {
    Cell::new(value.to_string())
}

pub fn colored_value_cell(value: impl fmt::Display, color: Color) -> Cell {
    Cell::new(value.to_string()).fg(color)
}

pub fn centered_table(table: &Table) -> String {
    center_block(&table.to_string())
}
