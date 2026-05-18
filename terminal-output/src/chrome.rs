use console::measure_text_width;
use crossterm::style::{Color, Stylize};
use crossterm::terminal::size;
use std::fmt::Write as _;
use textwrap::{wrap, Options};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Accent,
    Info,
    Success,
    Warning,
    Error,
}

pub fn tone_color(tone: Tone) -> Color {
    match tone {
        Tone::Accent => Color::Cyan,
        Tone::Info => Color::Blue,
        Tone::Success => Color::Green,
        Tone::Warning => Color::Yellow,
        Tone::Error => Color::Red,
    }
}

pub fn tone_tag(tone: Tone) -> &'static str {
    match tone {
        Tone::Accent => "ACCENT",
        Tone::Info => "INFO",
        Tone::Success => "OK",
        Tone::Warning => "WARN",
        Tone::Error => "ERR",
    }
}

pub fn terminal_width() -> usize {
    terminal_width_from_size(size().ok().map(|(columns, _)| columns))
}

pub fn terminal_width_from_size(columns: Option<u16>) -> usize {
    columns.map_or(100, usize::from)
}

pub fn banner(title: &str) -> String {
    let inner_width = terminal_width().saturating_sub(4).max(20);
    let lines = vec![title.to_uppercase()];
    center_block(&boxed_lines(
        Tone::Accent,
        &lines,
        inner_width,
        BoxAlignment::Center,
    ))
}

pub fn section_header(title: &str) -> String {
    let inner_width = terminal_width().saturating_sub(4).max(20);
    let lines = vec![title.to_string()];
    center_block(&boxed_lines(
        Tone::Accent,
        &lines,
        inner_width,
        BoxAlignment::Center,
    ))
}

pub fn section_footer() -> String {
    let inner_width = terminal_width().saturating_sub(4).max(20);
    center_block(&format!("╰{}╯", "─".repeat(inner_width + 2)))
}

pub fn status_line(tone: Tone, label: &str, detail: &str) -> String {
    center_block(&format!(
        "{} {}",
        label.with(tone_color(tone)).bold(),
        detail
    ))
}

pub fn notice_box(tone: Tone, title: &str, body: &str) -> String {
    let inner_width = terminal_width().saturating_sub(4).max(20);
    let mut lines = vec![format!("[{}] {}", tone_tag(tone), title)];
    lines.extend(wrap_text(body, inner_width));
    center_block(&boxed_lines(tone, &lines, inner_width, BoxAlignment::Left))
}

pub fn center_block(block: &str) -> String {
    center_block_with_width(block, terminal_width())
}

pub fn center_block_with_width(block: &str, width: usize) -> String {
    let lines: Vec<&str> = block.lines().collect();
    let block_width = lines
        .iter()
        .map(|line| measure_text_width(line))
        .max()
        .unwrap_or(0);
    let left_padding = width.saturating_sub(block_width) / 2;
    let padding = " ".repeat(left_padding);
    lines
        .into_iter()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("{padding}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if text.trim().is_empty() {
        return vec![String::new()];
    }
    let options = Options::new(width.max(20));
    let mut out = Vec::new();
    for (index, paragraph) in text.split('\n').enumerate() {
        if index > 0 {
            out.push(String::new());
        }
        if paragraph.trim().is_empty() {
            continue;
        }
        out.extend(
            wrap(paragraph, &options)
                .into_iter()
                .map(|line| line.into_owned()),
        );
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoxAlignment {
    Left,
    Center,
}

fn boxed_lines(
    tone: Tone,
    lines: &[String],
    inner_width: usize,
    alignment: BoxAlignment,
) -> String {
    let color = tone_color(tone);
    let border_top = format!("╭{}╮", "─".repeat(inner_width + 2));
    let border_bottom = format!("╰{}╯", "─".repeat(inner_width + 2));
    let mut out = String::new();
    let _ = writeln!(out, "{}", border_top.as_str().with(color));
    for line in lines {
        let visible_len = measure_text_width(line).min(inner_width);
        let padding = inner_width.saturating_sub(visible_len);
        let rendered = match alignment {
            BoxAlignment::Left => format!("{}{}", line, " ".repeat(padding)),
            BoxAlignment::Center => {
                let left = padding / 2;
                let right = padding - left;
                format!("{}{}{}", " ".repeat(left), line, " ".repeat(right))
            }
        };
        let _ = writeln!(out, "│ {} │", rendered);
    }
    let _ = writeln!(out, "{}", border_bottom.as_str().with(color));
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_should_include_title() {
        let out = banner("fastboot");
        assert!(out.contains("FASTBOOT"));
        assert!(!out.contains("guarded flashing"));
    }

    #[test]
    fn center_block_with_width_should_pad_each_line_by_visible_width() {
        let out = center_block_with_width("abc\nＨ", 7);

        assert_eq!(out, "  abc\n  Ｈ");
    }

    #[test]
    fn terminal_width_from_size_should_use_real_columns_without_clamping() {
        assert_eq!(terminal_width_from_size(Some(40)), 40);
        assert_eq!(terminal_width_from_size(Some(200)), 200);
        assert_eq!(terminal_width_from_size(None), 100);
    }

    #[test]
    fn section_header_should_be_title_only() {
        let out = section_header("FLASH PLAN");

        assert!(out.contains("FLASH PLAN"));
        assert!(!out.contains("ACCENT"));
        assert!(!out.contains("◆"));
        assert!(!out.contains("planned write"));
    }

    #[test]
    fn section_header_should_center_text_in_box() {
        let out = boxed_lines(
            Tone::Accent,
            &[String::from("FLASH PLAN")],
            20,
            BoxAlignment::Center,
        );

        assert!(out.contains("FLASH PLAN"));
        assert!(out.contains("╭"));
        assert!(out.contains("╰"));
    }

    #[test]
    fn section_footer_should_be_border_only() {
        let out = section_footer();

        assert!(out.contains("╰"));
        assert!(out.contains("╯"));
        assert!(!out.contains("FLASH PLAN"));
        assert!(!out.contains("ACCENT"));
    }

    #[test]
    fn status_line_should_not_include_tone_tag() {
        let out = status_line(Tone::Success, "device", "ready");

        assert!(!out.contains("OK"));
        assert!(out.contains("device"));
        assert!(out.contains("ready"));
    }

    #[test]
    fn notice_box_should_wrap_body_text() {
        let out = notice_box(
            Tone::Warning,
            "plan warning",
            "This is a long warning paragraph that should wrap rather than overflow the terminal width.",
        );

        assert!(out.contains("WARN"));
        assert!(out.contains("plan warning"));
        assert!(out.contains("wrap rather than overflow"));
    }
}
