//! Format-tool wrappers for building ext4/f2fs images and wiping userdata.

pub mod ext4;
pub mod f2fs;
pub mod tools;
pub mod wipe;

pub use tools::FormatTools;
pub use wipe::{
    detect_userdata, erase_optional_partition, format_userdata, format_userdata_with_info,
    generate_userdata_image, parse_fastboot_u64, wipe_data, wipe_data_with_info,
    FormatUserdataOptions, FormatUserdataOutcome, GeneratedUserdataImage, OptionalEraseOutcome,
    UserdataInfo, WipeDataOptions, WipeDataOutcome,
};