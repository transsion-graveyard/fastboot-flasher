use std::io::IsTerminal;

use anyhow::Context;
use cliclack::{confirm, intro, note, outro, outro_cancel};
use pawflash::{FlashPlan, FlashSummaryDto};
use textwrap::Options;

use crate::cli_app::{OutputFormat, UiMode};

pub struct Session {
    mode: UiMode,
    output: OutputFormat,
}

impl Session {
    pub fn new(mode: UiMode, output: OutputFormat) -> Self {
        Self { mode, output }
    }

    pub fn mode(&self) -> UiMode {
        self.mode
    }

    pub fn output(&self) -> OutputFormat {
        self.output
    }

    pub fn is_human(&self) -> bool {
        self.mode == UiMode::Human && std::io::stdout().is_terminal()
    }

    pub fn intro(&self, title: &str) -> anyhow::Result<()> {
        if self.is_human() {
            intro(title).context("render intro")?;
        }
        Ok(())
    }

    pub fn note(&self, title: &str, body: impl Into<String>) -> anyhow::Result<()> {
        if self.is_human() {
            note(title, wrap_for_note(&body.into())).context("render note")?;
        }
        Ok(())
    }

    pub fn confirm(&self, prompt: &str, default: bool) -> anyhow::Result<bool> {
        if self.mode == UiMode::Machine || !std::io::stdin().is_terminal() {
            return Ok(true);
        }

        confirm(prompt)
            .initial_value(default)
            .interact()
            .map_err(anyhow::Error::from)
            .context(prompt.to_string())
    }

    pub fn finish_success(&self, message: impl Into<String>) -> anyhow::Result<()> {
        if self.is_human() {
            outro(message.into()).context("render success outro")?;
        }
        Ok(())
    }

    pub fn finish_cancelled(&self, message: impl Into<String>) -> anyhow::Result<()> {
        if self.is_human() {
            outro_cancel(message.into()).context("render cancel outro")?;
        }
        Ok(())
    }

    pub fn emit_json<T: serde::Serialize>(&self, value: &T) -> anyhow::Result<()> {
        match self.output {
            OutputFormat::Json => {
                println!("{}", serde_json::to_string_pretty(value)?);
            }
            OutputFormat::Human => {}
        }
        Ok(())
    }

    pub fn render_plan_summary(&self, plan: &FlashPlan) -> anyhow::Result<()> {
        if !self.is_human() {
            return Ok(());
        }

        let total_bytes = plan
            .actions
            .iter()
            .map(|action| u64::try_from(action.size).unwrap_or(0))
            .sum::<u64>();
        let body = format!(
            "mode: {}\nstorage: {}\nslot policy: {}\nactions: {} (flash {}, wipe {}, skipped {})\ntotal: {}",
            plan.mode,
            plan.storage_selection,
            plan.slot_policy_effective,
            plan.actions.len(),
            plan.summary.flash_count,
            plan.summary.wipe_count,
            plan.summary.skipped_count,
            indicatif::HumanBytes(total_bytes)
        );
        self.note("Plan", body)
    }

    pub fn render_run_summary(&self, summary: &FlashSummaryDto) -> anyhow::Result<()> {
        if !self.is_human() {
            return Ok(());
        }

        self.note(
            "Result",
            format!(
                "flash: {}\nwipe: {}\nskipped: {}\ntotal: {}",
                summary.flash_count,
                summary.wipe_count,
                summary.skipped_count,
                indicatif::HumanBytes(summary.total_bytes)
            ),
        )
    }
}

fn wrap_for_note(message: &str) -> String {
    let terminal_width = terminal_output::chrome::terminal_width();
    let content_width = terminal_width.saturating_sub(8).clamp(20, 72);
    let options = Options::new(content_width)
        .break_words(false)
        .word_separator(textwrap::WordSeparator::AsciiSpace);

    message
        .split('\n')
        .map(|paragraph| {
            if paragraph.trim().is_empty() {
                String::new()
            } else {
                textwrap::fill(paragraph, &options)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::wrap_for_note;

    #[test]
    fn wrap_for_note_preserves_existing_line_breaks() {
        let wrapped = wrap_for_note("mode: dirty-flash\nstorage: auto");

        assert!(wrapped.contains("mode: dirty-flash"));
        assert!(wrapped.contains("storage: auto"));
    }

    #[test]
    fn wrap_for_note_wraps_long_single_lines() {
        let wrapped = wrap_for_note(
            "No fastboot device is ready. You can connect one now or let pawflash try force-fastboot.",
        );

        assert!(wrapped.contains('\n'));
        assert!(wrapped.lines().all(|line| line.len() < 80));
    }
}
