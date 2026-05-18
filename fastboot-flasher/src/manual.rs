use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use fastboot_rs::{partition_with_slot, resolve_slot_suffix, SlotSelection};

use crate::cli::SlotArg;

const EMPTY_VBMETA_IMAGE: &[u8] = include_bytes!("../assets/empty_vbmeta.img");

#[derive(Debug, Clone, PartialEq, Eq)]
enum ManualTarget {
    Exact(String),
    Slotted { base: String, slot: SlotSelection },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualFlashAction {
    pub partition: String,
    pub image: PathBuf,
    pub size: u64,
    pub reason: String,
    target: ManualTarget,
}

pub fn manual_flash_action(
    partition: impl Into<String>,
    image: impl Into<PathBuf>,
    slot: Option<SlotArg>,
) -> anyhow::Result<ManualFlashAction> {
    let partition = partition.into();
    let image = image.into();
    let metadata = std::fs::metadata(&image)
        .with_context(|| format!("read image metadata for {}", image.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("{} is not a regular file", image.display());
    }
    let (partition, target) = manual_target(partition, slot)?;
    Ok(ManualFlashAction {
        partition,
        image,
        size: metadata.len(),
        reason: "manual image".to_string(),
        target,
    })
}

pub fn manual_flash_actions(
    partition: impl Into<String>,
    image: impl Into<PathBuf>,
    slot: Option<SlotArg>,
) -> anyhow::Result<Vec<ManualFlashAction>> {
    let partition = partition.into();
    let image = image.into();

    if matches!(slot, Some(SlotArg::All)) {
        return Ok(vec![
            manual_flash_action(partition.clone(), image.clone(), Some(SlotArg::A))?,
            manual_flash_action(partition, image, Some(SlotArg::B))?,
        ]);
    }

    Ok(vec![manual_flash_action(partition, image, slot)?])
}

pub fn standalone_disable_vbmeta_path() -> anyhow::Result<PathBuf> {
    let path = std::env::temp_dir()
        .join("force-fastboot")
        .join("empty_vbmeta.img");
    if path.is_file() {
        let existing = std::fs::read(&path)
            .with_context(|| format!("read bundled vbmeta cache {}", path.display()))?;
        if existing == EMPTY_VBMETA_IMAGE {
            return Ok(path);
        }
    }
    let parent = path
        .parent()
        .context("compute bundled vbmeta cache directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create bundled vbmeta cache directory {}", parent.display()))?;
    std::fs::write(&path, EMPTY_VBMETA_IMAGE)
        .with_context(|| format!("write bundled vbmeta image to {}", path.display()))?;
    Ok(path)
}

pub fn disable_vbmeta_actions(empty_image: &Path) -> anyhow::Result<Vec<ManualFlashAction>> {
    let metadata = std::fs::metadata(empty_image)
        .with_context(|| format!("read image metadata for {}", empty_image.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("{} is not a regular file", empty_image.display());
    }
    Ok(["vbmeta_a", "vbmeta_b"]
        .into_iter()
        .map(|partition| ManualFlashAction {
            partition: partition.to_string(),
            image: empty_image.to_path_buf(),
            size: metadata.len(),
            reason: "disable-vbmeta empty image".to_string(),
            target: ManualTarget::Exact(partition.to_string()),
        })
        .collect())
}

impl ManualFlashAction {
    pub fn resolved_partition(&self, vars: &HashMap<String, String>) -> anyhow::Result<String> {
        match &self.target {
            ManualTarget::Exact(partition) => Ok(partition.clone()),
            ManualTarget::Slotted { base, slot } => {
                let suffix = resolve_slot_suffix(*slot, vars)
                    .with_context(|| format!("resolve slot for {base}"))?;
                Ok(partition_with_slot(base, &suffix))
            }
        }
    }
}

fn manual_target(
    partition: String,
    slot: Option<SlotArg>,
) -> anyhow::Result<(String, ManualTarget)> {
    let Some(slot) = slot else {
        return Ok((partition.clone(), ManualTarget::Exact(partition)));
    };

    if partition.ends_with("_a") || partition.ends_with("_b") {
        anyhow::bail!("{partition} already has a slot suffix");
    }

    match slot {
        SlotArg::A => {
            let resolved = partition_with_slot(&partition, "a");
            Ok((resolved.clone(), ManualTarget::Exact(resolved)))
        }
        SlotArg::B => {
            let resolved = partition_with_slot(&partition, "b");
            Ok((resolved.clone(), ManualTarget::Exact(resolved)))
        }
        SlotArg::Active => Ok((
            format!("{partition}_<active>"),
            ManualTarget::Slotted {
                base: partition,
                slot: SlotSelection::Active,
            },
        )),
        SlotArg::Inactive => Ok((
            format!("{partition}_<inactive>"),
            ManualTarget::Slotted {
                base: partition,
                slot: SlotSelection::Inactive,
            },
        )),
        SlotArg::All => anyhow::bail!("manual flash --slot only accepts a, b, active, or inactive"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_disable_vbmeta_path_should_return_existing_absolute_file() {
        let path = standalone_disable_vbmeta_path().unwrap();

        assert!(path.is_absolute());
        assert!(path.is_file(), "expected file at {}", path.display());
    }

    #[test]
    fn manual_target_should_render_active_placeholder() {
        let (partition, _) = manual_target("boot".to_string(), Some(SlotArg::Active)).unwrap();

        assert_eq!(partition, "boot_<active>");
    }
}
