#![cfg_attr(not(windows), deny(unsafe_code))]
#![doc = include_str!("../README.md")]

/// Higher-level fastboot operation executors.
pub mod executor;
/// Image inspection and download preparation.
pub mod image;
/// Scatter-compatible fastboot operation models.
pub mod operation;
/// Low-level fastboot protocol types and helpers.
pub mod protocol;
/// Android sparse image parsing and splitting.
pub mod sparse;
/// Fastboot transports.
pub mod transport;

pub use executor::{flash_prepared_image, FastbootExecutionError, FlashProgress};
pub use image::{
    prepare_image, write_transfer_payload, write_transfer_payload_with_progress, ImageKind,
    ImagePayloadError, ImagePreparationError, ImageSource, ImageTransfer, PreparedImage, RawRange,
};
pub use operation::{
    current_slot, erase_operation, flash_operation, parse_max_download_size, partition_with_slot,
    resolve_slot_suffix, OperationKind, OperationSequence, OperationStep, PartitionTarget,
    PlannedOperation, SlotResolutionError, SlotSelection,
};
pub use transport::{open_fastboot, open_fastboot_with_observer, BackendKind, FastbootDevice, FastbootError, FastbootOpenError, ProbeEvent, ProbeLogLevel};
