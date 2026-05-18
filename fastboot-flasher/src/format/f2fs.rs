use std::path::Path;
use std::process::Command;

use anyhow::Context;

use super::tools::FormatTools;

pub fn build_f2fs_image(
    tools: &FormatTools,
    output_img: &Path,
    userdata_size: u64,
    casefold: bool,
) -> anyhow::Result<()> {
    let binary = if casefold && tools.make_f2fs_casefold.is_file() {
        &tools.make_f2fs_casefold
    } else {
        &tools.make_f2fs
    };

    let mut cmd = Command::new(binary);
    cmd.arg("-S")
        .arg(userdata_size.to_string())
        .arg("-g")
        .arg("android")
        .arg(output_img);
    tools.apply_runtime_env(&mut cmd)?;

    let output = cmd.output().context("run make_f2fs")?;
    anyhow::ensure!(
        output.status.success(),
        "make_f2fs failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );

    Ok(())
}
