pub use terminal_output::progress::{
    centered_padding, centered_prefix, fit_width, fixed_bar_width, format_byte_pair, format_mm_ss,
    timed_style, visible_width,
};

/// One planned dry-run progress increment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DryRunStep {
    pub bytes: u64,
}

/// Build visible byte increments for a dry-run transfer simulation.
pub fn dry_run_steps(total_bytes: u64, speed_mib: u64) -> Vec<DryRunStep> {
    let bytes_per_second = speed_mib.saturating_mul(1024 * 1024).max(1);
    let target_frames = 10;
    let step = (bytes_per_second / target_frames).max(1);
    let visible_min_steps = 12;
    let step = if total_bytes > 0 {
        step.min((total_bytes / visible_min_steps).max(1))
    } else {
        1
    };
    let mut remaining = total_bytes.max(1);
    let mut out = Vec::new();
    while remaining > 0 {
        let bytes = remaining.min(step);
        out.push(DryRunStep { bytes });
        remaining -= bytes;
    }
    out
}

pub fn should_confirm_before_simulation(yes: bool) -> bool {
    !yes
}

pub fn selective_option_label(partition: &str, safety_class: &str, size_human: &str) -> String {
    format!("{partition} [{safety_class}] {size_human}")
}

pub fn max_visible_width<I, S>(items: I) -> usize
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    items
        .into_iter()
        .map(|item| visible_width(item.as_ref()))
        .max()
        .unwrap_or(0)
}

pub fn progress_header(summary: ActionSummary, dry_run: bool) -> String {
    let _ = dry_run;
    let noun = if summary.action_count() == 1 {
        "action"
    } else {
        "actions"
    };
    format!(
        "{} {noun} {}",
        summary.action_count(),
        indicatif::HumanBytes(summary.total_bytes)
    )
}

pub fn active_action_label(index: usize, total_count: usize) -> String {
    action_label(index, total_count)
}

pub fn flash_history_message(
    index: usize,
    total_count: usize,
    partition: &str,
    bytes: u64,
    target_width: usize,
) -> String {
    compact_history_message(index, total_count, "flash", partition, bytes, target_width)
}

pub fn skipped_flash_history_message(
    index: usize,
    total_count: usize,
    partition: &str,
    bytes: u64,
    target_width: usize,
) -> String {
    compact_history_message(
        index,
        total_count,
        "skipped flash",
        partition,
        bytes,
        target_width,
    )
}

pub fn erase_history_message(index: usize, total_count: usize, partition: &str) -> String {
    compact_history_message(index, total_count, "erase", partition, 0, 0)
}

pub fn skipped_erase_history_message(index: usize, total_count: usize, partition: &str) -> String {
    compact_history_message(index, total_count, "skipped erase", partition, 0, 0)
}

pub fn compact_history_message(
    index: usize,
    total_count: usize,
    action: &str,
    partition: &str,
    bytes: u64,
    target_width: usize,
) -> String {
    match (action, bytes) {
        ("flash", bytes) => history_with_size(
            &format!("{} {}", action_label(index, total_count), partition),
            &indicatif::HumanBytes(bytes).to_string(),
            target_width,
        ),
        ("skipped flash", bytes) => history_with_size(
            &format!(
                "{} skipped flash {}",
                action_label(index, total_count),
                partition
            ),
            &indicatif::HumanBytes(bytes).to_string(),
            target_width,
        ),
        (_, 0) => format!(
            "{} {} {}",
            action_label(index, total_count),
            action,
            partition
        ),
        _ => format!(
            "{} {} {} {}",
            action_label(index, total_count),
            action,
            partition,
            indicatif::HumanBytes(bytes)
        ),
    }
}

pub fn flash_history_min_width(
    index: usize,
    total_count: usize,
    partition: &str,
    bytes: u64,
) -> usize {
    history_with_size_min_width(
        &format!("{} {}", action_label(index, total_count), partition),
        &indicatif::HumanBytes(bytes).to_string(),
    )
}

pub fn skipped_flash_history_min_width(
    index: usize,
    total_count: usize,
    partition: &str,
    bytes: u64,
) -> usize {
    history_with_size_min_width(
        &format!(
            "{} skipped flash {}",
            action_label(index, total_count),
            partition
        ),
        &indicatif::HumanBytes(bytes).to_string(),
    )
}

fn history_with_size(left: &str, right: &str, target_width: usize) -> String {
    let fill = target_width
        .saturating_sub(visible_width(left) + visible_width(right) + 2)
        .max(1);
    format!("{left} {} {right}", "-".repeat(fill))
}

fn history_with_size_min_width(left: &str, right: &str) -> usize {
    visible_width(left) + visible_width(right) + 3
}

fn action_label(index: usize, total_count: usize) -> String {
    let width = total_count.to_string().len().max(2);
    format!("{:>width$}/{}", index + 1, total_count, width = width)
}

/// Compact action summary for terminal output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionSummary {
    pub flash_count: usize,
    pub wipe_count: usize,
    pub skipped_count: usize,
    pub total_bytes: u64,
}

impl ActionSummary {
    pub fn action_count(self) -> usize {
        self.flash_count + self.wipe_count + self.skipped_count
    }
}

pub fn action_summary<'a>(actions: impl IntoIterator<Item = (&'a str, u64)>) -> ActionSummary {
    let mut summary = ActionSummary {
        flash_count: 0,
        wipe_count: 0,
        skipped_count: 0,
        total_bytes: 0,
    };
    for (action, size) in actions {
        match action {
            "flash" => {
                summary.flash_count += 1;
                summary.total_bytes = summary.total_bytes.saturating_add(size);
            }
            "wipe" => {
                summary.wipe_count += 1;
                summary.total_bytes = summary.total_bytes.saturating_add(size);
            }
            "skip" => {
                summary.skipped_count += 1;
            }
            _ => {}
        }
    }
    summary
}
