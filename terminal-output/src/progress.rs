use console::measure_text_width;
use indicatif::{ProgressState, ProgressStyle};
use std::{fmt::Write, time::Duration};

const BYTE_PAIR_WIDTH: usize = 21;

pub fn fixed_bar_width(terminal_columns: u16) -> usize {
    const OVERHEAD: usize = 67;
    usize::from(terminal_columns)
        .saturating_sub(OVERHEAD)
        .clamp(10, 15)
}

pub fn visible_width(text: &str) -> usize {
    measure_text_width(text)
}

pub fn fit_width(available_width: usize, min_width: usize, max_width: usize) -> usize {
    if available_width < min_width {
        available_width
    } else {
        available_width.min(max_width)
    }
}

pub fn centered_padding(terminal_columns: usize, content_width: usize) -> usize {
    terminal_columns.saturating_sub(content_width) / 2
}

pub fn centered_prefix(label: &str, content_width: usize, terminal_columns: usize) -> String {
    format!(
        "{}{}",
        " ".repeat(centered_padding(terminal_columns, content_width)),
        label
    )
}

pub fn format_mm_ss(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    format!("{:02}:{:02}", total_seconds / 60, total_seconds % 60)
}

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

pub fn timed_style(template: &str) -> ProgressStyle {
    ProgressStyle::with_template(template)
        .expect("progress template is valid")
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
