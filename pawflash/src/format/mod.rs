//! Format-tool wrappers for building ext4/f2fs images and wiping userdata.

pub mod ext4;
pub mod f2fs;
pub mod tools;
pub mod wipe;

pub use tools::FormatTools;
pub use wipe::{
    detect_ext4_partition, detect_userdata, erase_optional_partition, format_userdata,
    format_userdata_with_info, generate_ext4_partition_image, generate_userdata_image,
    parse_fastboot_u64, prepare_partition_reset, wipe_data, wipe_data_with_info, Ext4PartitionInfo,
    FormatUserdataOptions, FormatUserdataOutcome, GeneratedUserdataImage, OptionalEraseOutcome,
    PartitionResetAction, PartitionResetInfo, PreparedPartitionReset, UserdataInfo,
    WipeDataOptions, WipeDataOutcome,
};
