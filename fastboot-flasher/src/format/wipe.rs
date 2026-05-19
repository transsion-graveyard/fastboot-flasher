use std::path::{Path, PathBuf};

use anyhow::Context;
use fastboot_rs::{transport::nusb::NusbFastBootError, FlashProgress};
use tempfile::TempDir;

use crate::{erase_one_partition, flash_one_partition, NusbFastBoot};

use super::{ext4::build_ext4_image, f2fs::build_f2fs_image, tools::FormatTools};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserdataInfo {
    pub fs_type: String,
    pub size: u64,
    pub max_download_size: Option<u64>,
    pub erase_block_size: Option<u64>,
    pub logical_block_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FormatUserdataOptions {
    pub erase_fallback: bool,
    pub casefold: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WipeDataOptions {
    pub erase_metadata: bool,
    pub erase_cache: bool,
    pub erase_fallback: bool,
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

#[derive(Debug)]
pub struct GeneratedUserdataImage {
    temp_dir: TempDir,
    path: PathBuf,
}

impl GeneratedUserdataImage {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn image_len(&self) -> anyhow::Result<u64> {
        Ok(std::fs::metadata(&self.path)
            .with_context(|| format!("read generated image metadata for {}", self.path.display()))?
            .len())
    }

    pub fn keepalive_dir(&self) -> &Path {
        self.temp_dir.path()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatUserdataOutcome {
    pub info: UserdataInfo,
    pub used_erase_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WipeDataOutcome {
    pub format: FormatUserdataOutcome,
    pub metadata_erased: bool,
    pub cache_erased: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionalEraseOutcome {
    Erased,
    Skipped { reason: String },
}

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

async fn get_var_optional(dev: &mut NusbFastBoot, name: &str) -> Option<String> {
    dev.get_var(name)
        .await
        .ok()
        .map(|value| value.trim().to_string())
}

pub async fn detect_userdata(dev: &mut NusbFastBoot) -> anyhow::Result<UserdataInfo> {
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

pub fn generate_userdata_image(
    tools: &FormatTools,
    info: &UserdataInfo,
    options: &FormatUserdataOptions,
) -> anyhow::Result<GeneratedUserdataImage> {
    tools.validate()?;

    let temp_dir = tempfile::Builder::new()
        .prefix("fastboot-flasher-format-")
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

pub async fn format_userdata(
    dev: &mut NusbFastBoot,
    tools: &FormatTools,
    options: &FormatUserdataOptions,
    on_progress: impl FnMut(FlashProgress),
) -> anyhow::Result<FormatUserdataOutcome> {
    let info = detect_userdata(dev).await?;
    format_userdata_with_info(dev, tools, info, options, on_progress).await
}

pub async fn format_userdata_with_info(
    dev: &mut NusbFastBoot,
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

pub async fn wipe_data(
    dev: &mut NusbFastBoot,
    tools: &FormatTools,
    options: &WipeDataOptions,
    on_progress: impl FnMut(FlashProgress),
) -> anyhow::Result<WipeDataOutcome> {
    let info = detect_userdata(dev).await?;
    wipe_data_with_info(dev, tools, info, options, on_progress).await
}

pub async fn wipe_data_with_info(
    dev: &mut NusbFastBoot,
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

pub async fn erase_optional_partition(
    dev: &mut NusbFastBoot,
    partition: &str,
) -> anyhow::Result<OptionalEraseOutcome> {
    match dev.erase(partition).await {
        Ok(()) => Ok(OptionalEraseOutcome::Erased),
        Err(NusbFastBootError::FastbootFailed(reason)) => {
            Ok(OptionalEraseOutcome::Skipped { reason })
        }
        Err(error) => Err(anyhow::Error::from(error)).with_context(|| format!("erase {partition}")),
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
