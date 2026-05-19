use std::{collections::HashMap, path::PathBuf};

use thiserror::Error;

use crate::{
    image::{ImageSource, PreparedImage},
    protocol::parse_u32,
};

/// A partition target with an optional resolved slot suffix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionTarget {
    /// Partition name as fastboot expects it.
    pub name: String,
}

impl PartitionTarget {
    /// Create a partition target from an exact fastboot partition name.
    pub fn exact(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

/// A planned fastboot operation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    /// Download image data, then flash the target partition.
    Flash,
    /// Erase the target partition.
    Erase,
}

/// A device-independent operation suitable for scatter-plan bridging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedOperation {
    /// Operation kind.
    pub kind: OperationKind,
    /// Target partition.
    pub target: PartitionTarget,
    /// Optional image source for flash operations.
    pub image: Option<ImageSource>,
}

/// Create a flash operation.
pub fn flash_operation(
    partition: impl Into<String>,
    image_path: impl Into<PathBuf>,
    partition_size: Option<u64>,
) -> PlannedOperation {
    let mut image = ImageSource::new(image_path);
    image.partition_size = partition_size;
    PlannedOperation {
        kind: OperationKind::Flash,
        target: PartitionTarget::exact(partition),
        image: Some(image),
    }
}

/// Create an erase operation.
pub fn erase_operation(partition: impl Into<String>) -> PlannedOperation {
    PlannedOperation {
        kind: OperationKind::Erase,
        target: PartitionTarget::exact(partition),
        image: None,
    }
}

/// One executable fastboot step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationStep {
    /// Send a `download:` command with the given byte count.
    Download {
        /// Number of bytes to download.
        bytes: u32,
    },
    /// Send a `flash:` command.
    Flash {
        /// Partition name to flash.
        partition: String,
    },
    /// Send an `erase:` command.
    Erase {
        /// Partition name to erase.
        partition: String,
    },
}

/// A device-independent sequence of fastboot protocol steps.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OperationSequence {
    /// Ordered steps.
    pub steps: Vec<OperationStep>,
}

impl OperationSequence {
    /// Build the download/flash sequence for a prepared image.
    pub fn for_prepared_flash(partition: &str, image: &PreparedImage) -> Self {
        let mut steps = Vec::with_capacity(image.transfers.len() * 2);
        for transfer in &image.transfers {
            steps.push(OperationStep::Download {
                bytes: transfer.download_size(),
            });
            steps.push(OperationStep::Flash {
                partition: partition.to_string(),
            });
        }
        Self { steps }
    }

    /// Build the erase sequence for a target partition.
    pub fn for_erase(partition: &str) -> Self {
        Self {
            steps: vec![OperationStep::Erase {
                partition: partition.to_string(),
            }],
        }
    }
}

impl crate::image::ImageTransfer {
    /// Number of bytes passed to `download:`.
    pub fn download_size(&self) -> u32 {
        match self {
            Self::Raw { download_size, .. } | Self::Sparse { download_size, .. } => *download_size,
        }
    }
}

/// Slot selection requested by a higher-level planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotSelection {
    /// Use slot A.
    A,
    /// Use slot B.
    B,
    /// Use the currently active slot.
    Active,
    /// Use the currently inactive slot.
    Inactive,
}

/// Slot resolution failures.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SlotResolutionError {
    /// Fastboot vars did not include `current-slot`.
    #[error("missing current-slot fastboot variable")]
    MissingCurrentSlot,
    /// The slot value is not recognized.
    #[error("unsupported slot value: {0}")]
    UnsupportedSlot(String),
}

/// Parse `max-download-size` from a fastboot variable value.
///
/// # Examples
///
/// ```
/// use fastboot_rs::parse_max_download_size;
///
/// assert_eq!(parse_max_download_size("0x1000").unwrap(), 4096);
/// assert_eq!(parse_max_download_size("4096").unwrap(), 4096);
/// ```
pub fn parse_max_download_size(value: &str) -> Result<u32, std::num::ParseIntError> {
    parse_u32(value)
}

/// Return the current slot from a map returned by `getvar:all`.
pub fn current_slot(vars: &HashMap<String, String>) -> Result<String, SlotResolutionError> {
    normalize_slot(
        vars.get("current-slot")
            .ok_or(SlotResolutionError::MissingCurrentSlot)?,
    )
}

/// Resolve a slot selection to a partition suffix (`a` or `b`).
pub fn resolve_slot_suffix(
    selection: SlotSelection,
    vars: &HashMap<String, String>,
) -> Result<String, SlotResolutionError> {
    match selection {
        SlotSelection::A => Ok("a".to_string()),
        SlotSelection::B => Ok("b".to_string()),
        SlotSelection::Active => current_slot(vars),
        SlotSelection::Inactive => {
            let active = current_slot(vars)?;
            match active.as_str() {
                "a" => Ok("b".to_string()),
                "b" => Ok("a".to_string()),
                _ => Err(SlotResolutionError::UnsupportedSlot(active)),
            }
        }
    }
}

/// Apply a resolved slot suffix to a base partition name.
///
/// # Examples
///
/// ```
/// use fastboot_rs::partition_with_slot;
///
/// assert_eq!(partition_with_slot("boot", "a"), "boot_a");
/// ```
pub fn partition_with_slot(base: &str, suffix: &str) -> String {
    format!("{base}_{suffix}")
}

fn normalize_slot(slot: &str) -> Result<String, SlotResolutionError> {
    match slot.trim().trim_start_matches('_').to_lowercase().as_str() {
        "a" => Ok("a".to_string()),
        "b" => Ok("b".to_string()),
        other => Err(SlotResolutionError::UnsupportedSlot(other.to_string())),
    }
}
