//! Progress bar helpers — bar width calculations, text measuring, formatting for
//! elapsed/remaining time and byte counts, and a pre-built `ProgressStyle` factory.

use std::{fmt::Write, time::Duration};

use console::measure_text_width;
use indicatif::{ProgressState, ProgressStyle};

const BYTE_PAIR_WIDTH: usize = 21;

/// Compute a clamped progress-bar width (10–15 columns) based on terminal width.
pub fn fixed_bar_width(terminal_columns: u16) -> usize {
    const OVERHEAD: usize = 67;
    usize::from(terminal_columns)
        .saturating_sub(OVERHEAD)
        .clamp(10, 15)
}

/// Return the visible (display) width of a string, accounting for wide Unicode.
pub fn visible_width(text: &str) -> usize {
    measure_text_width(text)
}

/// Clamp `available_width` between `min_width` and `max_width`, preferring to
/// shrink rather than grow when space is tight.
pub fn fit_width(available_width: usize, min_width: usize, max_width: usize) -> usize {
    if available_width < min_width {
        available_width
    } else {
        available_width.min(max_width)
    }
}

/// Compute left padding to center a `content_width`-wide block in the terminal.
pub fn centered_padding(terminal_columns: usize, content_width: usize) -> usize {
    terminal_columns.saturating_sub(content_width) / 2
}

/// Build a prefix string with a label left-padded to center it in the terminal.
pub fn centered_prefix(label: &str, content_width: usize, terminal_columns: usize) -> String {
    format!(
        "{}{}",
        " ".repeat(centered_padding(terminal_columns, content_width)),
        label
    )
}

/// Format a [`Duration`] as `MM:SS` (minutes and seconds only).
pub fn format_mm_ss(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    format!("{:02}:{:02}", total_seconds / 60, total_seconds % 60)
}

/// Format a byte-count pair (`pos / total`) with human-readable units at a fixed width.
pub fn format_byte_pair(bytes: u64, total_bytes: u64) -> String {
    format!(
        "{:>BYTE_PAIR_WIDTH$}",
        format!(
            "{}/{}",
            indicatif::HumanBytes(bytes),
            indicatif::HumanBytes(total_bytes)
        )
    )
}

/// Construct a [`ProgressStyle`] from a template, injecting custom keys for
/// `elapsed_mmss`, `eta_mmss`, and `byte_pair`.
pub fn timed_style(template: &str) -> ProgressStyle {
    ProgressStyle::with_template(template)
        .unwrap_or_else(|_| ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {wide_msg}").expect("fallback template is valid"))
        .with_key(
            "elapsed_mmss",
            |state: &ProgressState, out: &mut dyn Write| {
                let _ = write!(out, "{}", format_mm_ss(state.elapsed()));
            },
        )
        .with_key("eta_mmss", |state: &ProgressState, out: &mut dyn Write| {
            let _ = write!(out, "{}", format_mm_ss(state.eta()));
        })
        .with_key("byte_pair", |state: &ProgressState, out: &mut dyn Write| {
            let total = state.len().unwrap_or_else(|| state.pos());
            let _ = write!(out, "{}", format_byte_pair(state.pos(), total));
        })
        .progress_chars("=> ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_bar_width_should_be_clamped() {
        assert_eq!(fixed_bar_width(70), 10);
        assert_eq!(fixed_bar_width(80), 13);
        assert_eq!(fixed_bar_width(120), 15);
    }

    #[test]
    fn visible_width_should_count_unicode_by_display_width() {
        assert_eq!(visible_width("abc"), 3);
        assert_eq!(visible_width("Ｈ"), 2);
        assert_eq!(visible_width("👩‍🔧"), 2);
    }

    #[test]
    fn fit_width_should_shrink_to_fit_when_space_is_tight() {
        assert_eq!(fit_width(8, 10, 30), 8);
        assert_eq!(fit_width(12, 10, 30), 12);
        assert_eq!(fit_width(50, 10, 30), 30);
    }

    #[test]
    fn centered_prefix_should_left_pad_the_label() {
        let prefix = centered_prefix("1/20", 20, 40);

        assert_eq!(prefix, "          1/20");
    }

    #[test]
    fn format_mm_ss_should_use_minutes_and_seconds_only() {
        assert_eq!(format_mm_ss(Duration::from_secs(1)), "00:01");
        assert_eq!(format_mm_ss(Duration::from_secs(65)), "01:05");
    }

    #[test]
    fn format_byte_pair_should_use_fixed_width() {
        let tiny = format_byte_pair(1024 * 1024, 1024 * 1024);
        let large = format_byte_pair(9 * 1024 * 1024 * 1024, 9 * 1024 * 1024 * 1024);

        assert_eq!(tiny.len(), large.len());
        assert_eq!(tiny.trim(), "1.00 MiB/1.00 MiB");
        assert_eq!(large.trim(), "9.00 GiB/9.00 GiB");
    }
}
