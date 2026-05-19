//! A status spinner that gracefully degrades to a plain status line when stdout is
//! not a terminal.

use std::{io::IsTerminal, time::Duration};

use indicatif::ProgressBar;

use crate::chrome::{status_line, Tone};

/// A spinner that shows activity while a task runs. Falls back to a static status
/// line when stdout is not a terminal.
pub struct StatusSpinner {
    pb: Option<ProgressBar>,
}

impl StatusSpinner {
    /// Create a new spinner with the given status message.
    ///
    /// When stdout is a terminal an animated indicatif spinner is started;
    /// otherwise the message is printed as a plain status line.
    pub fn new(message: &str) -> Self {
        if std::io::stdout().is_terminal() {
            let pb = ProgressBar::new_spinner();
            pb.set_message(message.to_string());
            pb.enable_steady_tick(Duration::from_millis(100));
            Self { pb: Some(pb) }
        } else {
            eprintln!("{}", status_line(Tone::Info, "status", message));
            Self { pb: None }
        }
    }
}

impl Drop for StatusSpinner {
    fn drop(&mut self) {
        if let Some(pb) = &self.pb {
            pb.finish_and_clear();
        }
    }
}
