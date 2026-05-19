use std::{io::IsTerminal, time::Duration};

use indicatif::ProgressBar;

use crate::chrome::{status_line, Tone};

pub struct StatusSpinner {
    pb: Option<ProgressBar>,
}

impl StatusSpinner {
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
