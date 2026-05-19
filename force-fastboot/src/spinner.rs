//!
//! Re-exports [`StatusSpinner`] from the `terminal-output` crate for convenience.

/// A spinner shown during long-running operations. Disappears when dropped.
pub use terminal_output::spinner::StatusSpinner;
