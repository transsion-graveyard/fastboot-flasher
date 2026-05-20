use std::{collections::HashMap, io::IsTerminal, time::Duration};

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressFinish};
use pawflash::domain::{FlashEvent, FlashOperation, FlashSummaryDto};
use pawflash::progress::{active_action_label, completed_total_style, history_row_style};

const BAR_REFRESH_HZ: u8 = 10;

/// CLI progress renderer that uses indicatif on terminals and falls back to
/// plain text when output is redirected.
pub struct CliProgressRenderer {
    interactive: bool,
    mp: Option<MultiProgress>,
    overall: Option<ProgressBar>,
    rows: HashMap<String, RowState>,
    next_index: usize,
    total_count: usize,
    bar_width: usize,
    message_width: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionKind {
    Flash,
    Format,
    SimulateFlash,
    SimulateFormat,
    Erase,
}

struct RowState {
    bar: ProgressBar,
    index: usize,
    kind: ActionKind,
    partition: String,
    bytes: u64,
    total: u64,
    total_count: usize,
    finished: bool,
}

impl CliProgressRenderer {
    /// Create a renderer for the current stdout target.
    pub fn new() -> Self {
        let interactive = std::io::stdout().is_terminal();
        let (bar_width, message_width) = compact_layout();
        let mp = interactive.then(|| {
            MultiProgress::with_draw_target(ProgressDrawTarget::stdout_with_hz(BAR_REFRESH_HZ))
        });

        Self {
            interactive,
            mp,
            overall: None,
            rows: HashMap::new(),
            next_index: 0,
            total_count: 0,
            bar_width,
            message_width,
        }
    }

    /// Handle a flash event.
    pub fn handle(&mut self, event: &FlashEvent) -> bool {
        if !self.interactive {
            return false;
        }

        match event {
            FlashEvent::WaitingForDevice
            | FlashEvent::DeviceCheckDiagnostic { .. }
            | FlashEvent::GsiStatus { .. }
            | FlashEvent::Rebooting { .. } => {
                self.log_line(event_text(event));
            }
            FlashEvent::PlanBuilt {
                actions,
                total_bytes,
            } => {
                self.total_count = *actions;
                self.ensure_overall(*total_bytes);
                for row in self.rows.values_mut() {
                    row.total_count = self.total_count.max(row.total_count).max(row.index + 1);
                    row.bar
                        .set_prefix(active_action_label(row.index, row.total_count));
                }
                self.log_line(format!(
                    "plan built: {actions} actions, {}",
                    indicatif::HumanBytes(*total_bytes)
                ));
            }
            FlashEvent::PreparingImage {
                partition,
                operation,
            } => {
                self.ensure_row(partition, operation_kind(*operation, false), 1);
            }
            FlashEvent::Flashing {
                partition,
                operation,
                bytes,
                total,
                ..
            } => {
                self.update_row(partition, operation_kind(*operation, false), *bytes, *total);
            }
            FlashEvent::Simulating {
                partition,
                operation,
                bytes,
                total,
                ..
            } => {
                self.update_row(partition, operation_kind(*operation, true), *bytes, *total);
            }
            FlashEvent::Erasing { partition } => {
                self.ensure_row(partition, ActionKind::Erase, 1);
            }
            FlashEvent::EraseComplete { partition }
            | FlashEvent::PartitionComplete { partition, .. } => {
                self.finish_row(partition, RowOutcome::Complete);
            }
            FlashEvent::PartitionSkipped {
                partition, reason, ..
            } => {
                self.finish_row(partition, RowOutcome::Skipped(reason.clone()));
            }
            FlashEvent::PartitionFailed {
                partition, error, ..
            } => {
                self.finish_row(partition, RowOutcome::Failed(error.clone()));
            }
            FlashEvent::Overall { bytes, total } => {
                self.update_overall(*bytes, *total);
            }
            FlashEvent::Complete { summary } => {
                self.finish_summary(summary);
            }
            FlashEvent::Cancelled { message } | FlashEvent::Error { message } => {
                self.log_line(message.clone());
                self.finish_open_rows(message);
            }
        }

        true
    }

    fn ensure_overall(&mut self, total_bytes: u64) {
        let Some(mp) = &self.mp else {
            return;
        };

        if self.overall.is_none() {
            let bar = mp.add(ProgressBar::new(total_bytes.max(1)));
            bar.set_prefix("overall".to_string());
            bar.set_style(completed_total_style(self.bar_width));
            bar.enable_steady_tick(Duration::from_millis(100));
            self.overall = Some(bar.with_finish(ProgressFinish::AndLeave));
        }
    }

    fn update_overall(&mut self, bytes: u64, total: u64) {
        self.ensure_overall(total);
        if let Some(bar) = &self.overall {
            bar.set_length(total.max(1));
            bar.set_position(bytes.min(total.max(1)));
        }
    }

    fn ensure_row(&mut self, partition: &str, kind: ActionKind, total: u64) {
        if self.rows.contains_key(partition) {
            return;
        }

        let Some(mp) = &self.mp else {
            return;
        };

        let index = self.next_index;
        self.next_index += 1;
        let pb = if let Some(ref overall_bar) = self.overall {
            mp.insert_before(overall_bar, ProgressBar::new(total.max(1)))
        } else {
            mp.add(ProgressBar::new(total.max(1)))
        };
        pb.set_style(active_row_style(self.bar_width, self.message_width));
        pb.set_prefix(active_action_label(index, self.total_count.max(index + 1)));
        pb.set_message(partition.to_string());
        pb.enable_steady_tick(Duration::from_millis(100));
        pb.set_position(0);

        self.rows.insert(
            partition.to_string(),
            RowState {
                bar: pb.with_finish(ProgressFinish::AndLeave),
                index,
                kind,
                partition: partition.to_string(),
                bytes: 0,
                total: total.max(1),
                total_count: self.total_count.max(index + 1),
                finished: false,
            },
        );
    }

    fn update_row(&mut self, partition: &str, kind: ActionKind, bytes: u64, total: u64) {
        if self.rows.contains_key(partition) {
            if let Some(row) = self.rows.get_mut(partition) {
                if row.finished {
                    return;
                }
                row.kind = kind;
                row.bytes = bytes;
                row.total = total.max(1);
                row.total_count = self.total_count.max(row.total_count).max(row.index + 1);
                row.bar.set_length(row.total);
                row.bar.set_position(bytes.min(row.total));
                row.bar.set_message(row.partition.clone());
            }
            return;
        }

        self.ensure_row(partition, kind, total);
        if let Some(row) = self.rows.get_mut(partition) {
            row.kind = kind;
            row.bytes = bytes;
            row.total = total.max(1);
            row.total_count = self.total_count.max(row.total_count).max(row.index + 1);
            row.bar.set_length(row.total);
            row.bar.set_position(bytes.min(row.total));
        }
    }

    fn finish_row(&mut self, partition: &str, outcome: RowOutcome) {
        let Some(row) = self.rows.get_mut(partition) else {
            return;
        };
        if row.finished {
            return;
        }
        row.finished = true;
        row.bar.set_length(row.total.max(1));
        row.bar.set_position(row.total.max(1));
        row.bar.set_style(history_row_style(self.message_width));
        if matches!(
            row.kind,
            ActionKind::Flash
                | ActionKind::Format
                | ActionKind::SimulateFlash
                | ActionKind::SimulateFormat
        ) {
            row.bytes = row.total;
        }
        let message = match outcome {
            RowOutcome::Complete => complete_row_message(row),
            RowOutcome::Skipped(reason) => skipped_row_message(row, &reason),
            RowOutcome::Failed(error) => failed_row_message(row, &error),
        };
        row.bar.finish_with_message(message);
    }

    fn finish_open_rows(&mut self, message: &str) {
        for row in self.rows.values_mut() {
            if row.finished {
                continue;
            }
            row.finished = true;
            row.bar.set_style(history_row_style(self.message_width));
            row.bar.finish_with_message(format!(
                "{} {}",
                active_action_label(row.index, row.total_count),
                message
            ));
        }

        if let Some(bar) = &self.overall {
            bar.finish_with_message(message.to_string());
        }
    }

    fn finish_summary(&mut self, summary: &FlashSummaryDto) {
        self.log_line(complete_summary_line(summary));
        if let Some(bar) = &self.overall {
            bar.set_position(bar.length().unwrap_or(1));
            bar.finish();
        }
    }

    fn log_line(&self, line: String) {
        if let Some(mp) = &self.mp {
            let _ = mp.println(line);
        }
    }
}

enum RowOutcome {
    Complete,
    Skipped(String),
    Failed(String),
}

fn active_row_style(bar_width: usize, message_width: usize) -> indicatif::ProgressStyle {
    let template = format!(
        "{{prefix}} {{spinner:.green}} [{{bar:{bar_width}.cyan/black}}] {{byte_pair}} {{elapsed_mmss}} {{msg:<{message_width}}}"
    );
    terminal_output::progress::timed_style(&template)
}

fn complete_summary_line(summary: &FlashSummaryDto) -> String {
    format!(
        "done: flash={} wipe={} skipped={} total={}",
        summary.flash_count,
        summary.wipe_count,
        summary.skipped_count,
        indicatif::HumanBytes(summary.total_bytes)
    )
}

fn complete_row_message(row: &RowState) -> String {
    match row.kind {
        ActionKind::Flash
        | ActionKind::Format
        | ActionKind::SimulateFlash
        | ActionKind::SimulateFormat => completed_line(
            row.index,
            row.total_count,
            row.kind_label(),
            &row.partition,
            row.bytes,
        ),
        ActionKind::Erase => format!(
            "{} {} {}",
            action_label(row.index, row.total_count),
            row.kind_label(),
            row.partition
        ),
    }
}

fn skipped_row_message(row: &RowState, reason: &str) -> String {
    let base = match row.kind {
        ActionKind::Erase => "skipped erase",
        ActionKind::Flash | ActionKind::SimulateFlash => "skipped flash",
        ActionKind::Format | ActionKind::SimulateFormat => "skipped format",
    };
    format!(
        "{} {} {}: {}",
        action_label(row.index, row.total_count),
        base,
        row.partition,
        reason
    )
}

fn failed_row_message(row: &RowState, error: &str) -> String {
    format!(
        "{} {} {}: {}",
        action_label(row.index, row.total_count),
        row.kind_label(),
        row.partition,
        error
    )
}

fn completed_line(
    index: usize,
    total_count: usize,
    action: &str,
    partition: &str,
    bytes: u64,
) -> String {
    format!(
        "{} {} {} {}",
        action_label(index, total_count),
        action,
        partition,
        indicatif::HumanBytes(bytes)
    )
}

fn action_label(index: usize, total_count: usize) -> String {
    let width = total_count.to_string().len().max(2);
    format!("{:>width$}/{}", index + 1, total_count, width = width)
}

fn compact_layout() -> (usize, usize) {
    let columns = terminal_output::chrome::terminal_width();
    let bar_width = terminal_output::progress::bar_width(columns.min(u16::MAX as usize) as u16);
    let message_width = terminal_output::progress::fit_width(columns.saturating_sub(42), 8, 16);
    (bar_width, message_width)
}

impl RowState {
    fn kind_label(&self) -> &'static str {
        match self.kind {
            ActionKind::Flash | ActionKind::SimulateFlash => "flash",
            ActionKind::Format | ActionKind::SimulateFormat => "format",
            ActionKind::Erase => "erase",
        }
    }
}

fn operation_kind(operation: FlashOperation, simulated: bool) -> ActionKind {
    match (operation, simulated) {
        (FlashOperation::Flash, false) => ActionKind::Flash,
        (FlashOperation::Flash, true) => ActionKind::SimulateFlash,
        (FlashOperation::FormatUserdata, false) => ActionKind::Format,
        (FlashOperation::FormatUserdata, true) => ActionKind::SimulateFormat,
        (FlashOperation::Erase, _) => ActionKind::Erase,
    }
}

fn event_text(event: &FlashEvent) -> String {
    match event {
        FlashEvent::WaitingForDevice => "waiting for device".to_string(),
        FlashEvent::DeviceCheckDiagnostic {
            stage,
            level,
            message,
        } => format!("[{level}] {stage}: {message}"),
        FlashEvent::GsiStatus { status } => format!("gsi: {status}"),
        FlashEvent::Rebooting { target } => format!("rebooting to {target}"),
        FlashEvent::Cancelled { message } | FlashEvent::Error { message } => message.clone(),
        FlashEvent::PlanBuilt {
            actions,
            total_bytes,
        } => {
            format!(
                "plan built: {actions} actions, {}",
                indicatif::HumanBytes(*total_bytes)
            )
        }
        FlashEvent::Complete { summary } => complete_summary_line(summary),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_flash_line_should_be_compact() {
        let line = completed_line(0, 1, "flash", "super_long_partition_name", 1024 * 1024);

        assert_eq!(line, " 1/1 flash super_long_partition_name 1.00 MiB");
    }

    #[test]
    fn skipped_line_should_include_status_and_partition() {
        let row = RowState {
            bar: ProgressBar::hidden(),
            index: 1,
            kind: ActionKind::Flash,
            partition: "userdata".to_string(),
            bytes: 4096,
            total: 4096,
            total_count: 2,
            finished: false,
        };
        let line = skipped_row_message(&row, "no image");

        assert_eq!(line, " 2/2 skipped flash userdata: no image");
    }

    #[test]
    fn erase_completion_line_should_not_include_bytes() {
        let row = RowState {
            bar: ProgressBar::hidden(),
            index: 0,
            kind: ActionKind::Erase,
            partition: "metadata".to_string(),
            bytes: 0,
            total: 1,
            total_count: 3,
            finished: false,
        };
        let line = complete_row_message(&row);

        assert_eq!(line, " 1/3 erase metadata");
    }

    #[test]
    fn summary_line_should_report_totals() {
        let line = complete_summary_line(&FlashSummaryDto {
            flash_count: 2,
            wipe_count: 1,
            skipped_count: 3,
            total_bytes: 4 * 1024 * 1024,
        });

        assert_eq!(line, "done: flash=2 wipe=1 skipped=3 total=4.00 MiB");
    }
}
