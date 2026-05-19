//! Userdata detection, formatted image generation, and wipe operations.

use std::path::{Path, PathBuf};

use anyhow::Context;
use fastboot_rs::{FastbootDevice, FastbootError, FlashProgress};
use tempfile::TempDir;

use crate::{erase_one_partition, flash_one_partition};

use super::{ext4::build_ext4_image, f2fs::build_f2fs_image, tools::FormatTools};

/// Information about the userdata partition on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserdataInfo {
    /// Filesystem type (e.g. "ext4", "f2fs", "raw").
    pub fs_type: String,
    /// Partition size in bytes.
    pub size: u64,
    /// Maximum download size reported by the device (optional).
    pub max_download_size: Option<u64>,
    /// Erase block size (optional).
    pub erase_block_size: Option<u64>,
    /// Logical block size (optional).
    pub logical_block_size: Option<u64>,
}

/// Options for formatting the userdata partition.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FormatUserdataOptions {
    /// Fall back to `fastboot erase userdata` if image generation fails.
    pub erase_fallback: bool,
    /// Enable f2fs casefolding support.
    pub casefold: bool,
}

/// Options for the full wipe-data flow (userdata + optional partitions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WipeDataOptions {
    /// Also erase the metadata partition.
    pub erase_metadata: bool,
    /// Also erase the cache partition.
    pub erase_cache: bool,
    /// Fall back to `fastboot erase` if image generation fails.
    pub erase_fallback: bool,
    /// Enable f2fs casefolding when generating the userdata image.
    pub casefold: bool,
}

impl Default for WipeDataOptions {
    fn default() -> Self {
        Self {
            erase_metadata: true,
            erase_cache: true,
            erase_fallback: false,
            casefold: false,
        }
    }
}

/// A generated userdata filesystem image backed by a temporary directory.
#[derive(Debug)]
pub struct GeneratedUserdataImage {
    temp_dir: TempDir,
    path: PathBuf,
}

impl GeneratedUserdataImage {
    /// Path to the generated userdata image file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return the length of the generated image file in bytes.
    pub fn image_len(&self) -> anyhow::Result<u64> {
        Ok(std::fs::metadata(&self.path)
            .with_context(|| format!("read generated image metadata for {}", self.path.display()))?
            .len())
    }

    /// Path to the temporary directory keeping the image alive.
    pub fn keepalive_dir(&self) -> &Path {
        self.temp_dir.path()
    }
}

/// Outcome of a `format_userdata` operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatUserdataOutcome {
    /// The userdata partition info that was used.
    pub info: UserdataInfo,
    /// Whether the operation fell back to `fastboot erase userdata`.
    pub used_erase_fallback: bool,
}

/// Outcome of a full wipe-data operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WipeDataOutcome {
    /// The outcome of the userdata format.
    pub format: FormatUserdataOutcome,
    /// Whether metadata was erased.
    pub metadata_erased: bool,
    /// Whether cache was erased.
    pub cache_erased: bool,
}

/// Outcome of an optional partition erase (may be skipped on failure).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionalEraseOutcome {
    /// The partition was successfully erased.
    Erased,
    /// The erase was skipped (e.g. partition does not exist).
    Skipped {
        /// Reason why the erase was skipped.
        reason: String,
    },
}

/// Parse a u64 from a fastboot variable string (decimal, `0x`-prefixed hex, or
/// bare hex).
pub fn parse_fastboot_u64(value: &str) -> anyhow::Result<u64> {
    let value = value.trim();
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        Ok(u64::from_str_radix(hex, 16)?)
    } else if value.chars().all(|ch| ch.is_ascii_hexdigit())
        && value.chars().any(|ch| ch.is_ascii_alphabetic())
    {
        Ok(u64::from_str_radix(value, 16)?)
    } else {
        Ok(value.parse::<u64>()?)
    }
}

async fn get_var_optional(dev: &mut FastbootDevice, name: &str) -> Option<String> {
    dev.get_var_optional(name)
        .await
        .ok()
        .flatten()
        .map(|value| value.trim().to_string())
}

/// Read userdata partition info (filesystem type, size, block sizes) from the
/// device.
pub async fn detect_userdata(dev: &mut FastbootDevice) -> anyhow::Result<UserdataInfo> {
    let fs_type = dev
        .get_var("partition-type:userdata")
        .await
        .map(|value| value.trim().to_ascii_lowercase())
        .map_err(anyhow::Error::from)
        .context("get partition-type:userdata")?;
    let raw_size = dev
        .get_var("partition-size:userdata")
        .await
        .map_err(anyhow::Error::from)
        .context("get partition-size:userdata")?;
    let size = parse_fastboot_u64(&raw_size)
        .with_context(|| format!("parse partition-size:userdata ({raw_size})"))?;

    let max_download_size = get_var_optional(dev, "max-download-size")
        .await
        .and_then(|value| parse_fastboot_u64(&value).ok());
    let erase_block_size = get_var_optional(dev, "erase-block-size")
        .await
        .and_then(|value| parse_fastboot_u64(&value).ok());
    let logical_block_size = get_var_optional(dev, "logical-block-size")
        .await
        .and_then(|value| parse_fastboot_u64(&value).ok());

    Ok(UserdataInfo {
        fs_type,
        size,
        max_download_size,
        erase_block_size,
        logical_block_size,
    })
}

/// Generate a formatted userdata image (ext4 or f2fs) in a temporary
/// directory.
pub fn generate_userdata_image(
    tools: &FormatTools,
    info: &UserdataInfo,
    options: &FormatUserdataOptions,
) -> anyhow::Result<GeneratedUserdataImage> {
    tools.validate()?;

    let temp_dir = tempfile::Builder::new()
        .prefix("pawflash-format-")
        .tempdir()
        .context("create temp directory for userdata image")?;
    let path = temp_dir.path().join("userdata.img");

    match info.fs_type.as_str() {
        "ext4" => build_ext4_image(
            tools,
            &path,
            info.size,
            info.erase_block_size,
            info.logical_block_size,
        )?,
        "f2fs" => build_f2fs_image(tools, &path, info.size, options.casefold)?,
        other => anyhow::bail!("unsupported userdata filesystem type: {other}"),
    }

    Ok(GeneratedUserdataImage { temp_dir, path })
}

/// Detect userdata info, generate a formatted image, and flash it to the
/// device.
pub async fn format_userdata(
    dev: &mut FastbootDevice,
    tools: &FormatTools,
    options: &FormatUserdataOptions,
    on_progress: impl FnMut(FlashProgress),
) -> anyhow::Result<FormatUserdataOutcome> {
    let info = detect_userdata(dev).await?;
    format_userdata_with_info(dev, tools, info, options, on_progress).await
}

/// Format userdata using a pre-fetched [`UserdataInfo`], generating an image
/// and flashing it.
pub async fn format_userdata_with_info(
    dev: &mut FastbootDevice,
    tools: &FormatTools,
    info: UserdataInfo,
    options: &FormatUserdataOptions,
    on_progress: impl FnMut(FlashProgress),
) -> anyhow::Result<FormatUserdataOutcome> {
    let generated = match generate_userdata_image(tools, &info, options) {
        Ok(image) => image,
        Err(_error) if options.erase_fallback => {
            erase_one_partition(dev, "userdata")
                .await
                .context("erase userdata fallback")?;
            return Ok(FormatUserdataOutcome {
                info,
                used_erase_fallback: true,
            });
        }
        Err(error) => return Err(error).context("generate userdata image"),
    };

    let max_download = info
        .max_download_size
        .context("missing userdata max-download-size")?;
    let max_download = u32::try_from(max_download)
        .context("userdata max-download-size exceeds supported range")?;

    flash_one_partition(dev, "userdata", generated.path(), max_download, on_progress)
        .await
        .context("flash generated userdata image")?;

    Ok(FormatUserdataOutcome {
        info,
        used_erase_fallback: false,
    })
}

/// Full wipe flow: detect userdata, format it, and optionally erase metadata
/// and cache.
pub async fn wipe_data(
    dev: &mut FastbootDevice,
    tools: &FormatTools,
    options: &WipeDataOptions,
    on_progress: impl FnMut(FlashProgress),
) -> anyhow::Result<WipeDataOutcome> {
    let info = detect_userdata(dev).await?;
    wipe_data_with_info(dev, tools, info, options, on_progress).await
}

/// Full wipe flow using a pre-fetched [`UserdataInfo`].
pub async fn wipe_data_with_info(
    dev: &mut FastbootDevice,
    tools: &FormatTools,
    info: UserdataInfo,
    options: &WipeDataOptions,
    on_progress: impl FnMut(FlashProgress),
) -> anyhow::Result<WipeDataOutcome> {
    let format = format_userdata_with_info(
        dev,
        tools,
        info,
        &FormatUserdataOptions {
            erase_fallback: options.erase_fallback,
            casefold: options.casefold,
        },
        on_progress,
    )
    .await?;

    let metadata_erased = if options.erase_metadata {
        matches!(
            erase_optional_partition(dev, "metadata").await?,
            OptionalEraseOutcome::Erased
        )
    } else {
        false
    };
    let cache_erased = if options.erase_cache {
        matches!(
            erase_optional_partition(dev, "cache").await?,
            OptionalEraseOutcome::Erased
        )
    } else {
        false
    };

    Ok(WipeDataOutcome {
        format,
        metadata_erased,
        cache_erased,
    })
}

/// Erase a partition, treating "fastboot command failed" errors as
/// skippable rather than fatal.
pub async fn erase_optional_partition(
    dev: &mut FastbootDevice,
    partition: &str,
) -> anyhow::Result<OptionalEraseOutcome> {
    match dev.erase(partition).await {
        Ok(()) => Ok(OptionalEraseOutcome::Erased),
        Err(error) if is_skippable_fastboot_error(&error) => Ok(OptionalEraseOutcome::Skipped {
            reason: error.to_string(),
        }),
        Err(error) => Err(anyhow::Error::from(error)).with_context(|| format!("erase {partition}")),
    }
}

fn is_skippable_fastboot_error(error: &FastbootError) -> bool {
    match error {
        FastbootError::Nusb(fastboot_rs::transport::nusb::NusbFastBootError::FastbootFailed(_)) => {
            true
        }
        #[cfg(windows)]
        FastbootError::AdbWinApi(
            fastboot_rs::transport::adbwinapi::AdbWinApiFastbootError::FastbootFailed(_),
        ) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_fastboot_u64;

    #[test]
    fn parse_fastboot_u64_should_accept_decimal() {
        assert_eq!(parse_fastboot_u64("4096").unwrap(), 4096);
    }

    #[test]
    fn parse_fastboot_u64_should_accept_hex() {
        assert_eq!(parse_fastboot_u64("0x1000").unwrap(), 4096);
    }

    #[test]
    fn parse_fastboot_u64_should_accept_uppercase_hex_prefix() {
        assert_eq!(parse_fastboot_u64("0X1000").unwrap(), 4096);
    }

    #[test]
    fn parse_fastboot_u64_should_accept_unprefixed_hex_values() {
        assert_eq!(parse_fastboot_u64("7f000000").unwrap(), 0x7f000000);
        assert_eq!(parse_fastboot_u64("ABCDEF").unwrap(), 0xabcdef);
    }
}
