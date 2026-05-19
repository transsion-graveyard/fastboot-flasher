//! Build an ext4 userdata image using `mke2fs`.

use std::path::Path;
use std::process::Command;

use anyhow::Context;

use super::tools::FormatTools;

/// Build an ext4 image for userdata using the bundled `mke2fs` binary.
pub fn build_ext4_image(
    tools: &FormatTools,
    output_img: &Path,
    userdata_size: u64,
    erase_block_size: Option<u64>,
    logical_block_size: Option<u64>,
) -> anyhow::Result<()> {
    let block_size = 4096u64;
    let block_count = userdata_size / block_size;
    anyhow::ensure!(block_count > 0, "userdata partition is too small for ext4");

    let mut ext_opts = String::from("android_sparse");

    if let Some(erase) = erase_block_size {
        let stride = erase / block_size;
        if stride > 0 {
            ext_opts.push_str(&format!(",stride={stride}"));
        }
    }

    if let Some(logical) = logical_block_size {
        let stripe_width = logical / block_size;
        if stripe_width > 0 {
            ext_opts.push_str(&format!(",stripe-width={stripe_width}"));
        }
    }

    let mut cmd = Command::new(&tools.mke2fs);
    cmd.arg("-t")
        .arg("ext4")
        .arg("-b")
        .arg("4096")
        .arg("-E")
        .arg(ext_opts)
        .arg("-O")
        .arg("uninit_bg")
        .arg(output_img)
        .arg(block_count.to_string())
        .env("MKE2FS_CONFIG", &tools.mke2fs_conf);
    tools.apply_runtime_env(&mut cmd)?;

    let output = cmd.output().context("run mke2fs")?;
    anyhow::ensure!(
        output.status.success(),
        "mke2fs failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );

    Ok(())
}
