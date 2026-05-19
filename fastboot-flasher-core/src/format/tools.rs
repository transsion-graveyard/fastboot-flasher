//! Paths to bundled format tool binaries and helpers.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Paths to the bundled filesystem-formatting tools.
pub struct FormatTools {
    /// Root directory containing platform-specific subdirectories.
    pub root: PathBuf,
    /// Platform-specific directory with tool binaries.
    pub dir: PathBuf,
    /// Path to the `mke2fs` binary.
    pub mke2fs: PathBuf,
    /// Path to the `make_f2fs` binary.
    pub make_f2fs: PathBuf,
    /// Path to the `make_f2fs_casefold` binary (optional).
    pub make_f2fs_casefold: PathBuf,
    /// Path to the `mke2fs.conf` configuration file.
    pub mke2fs_conf: PathBuf,
}

impl FormatTools {
    /// Build [`FormatTools`] from a bin root directory, auto-detecting the
    /// platform subdirectory.
    ///
    /// # Errors
    ///
    /// Returns an error on unsupported hosts.
    pub fn from_bin_root(root: &Path) -> anyhow::Result<Self> {
        #[cfg(target_os = "windows")]
        let platform = "windows";
        #[cfg(target_os = "linux")]
        let platform = "linux";
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        anyhow::bail!("format tools are only supported on Linux and Windows hosts");

        Ok(Self::from_platform_root(root, platform))
    }

    /// Build [`FormatTools`] pointing at the crate's built-in asset binaries.
    ///
    /// # Errors
    ///
    /// Returns an error if the host platform is unsupported.
    pub fn from_cli_assets() -> anyhow::Result<Self> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("bin");
        Self::from_bin_root(&root)
    }

    pub(crate) fn from_platform_root(root: &Path, platform: &str) -> Self {
        let dir = root.join(platform);
        let exe = if platform == "windows" { ".exe" } else { "" };
        Self {
            root: root.to_path_buf(),
            mke2fs: dir.join(format!("mke2fs{exe}")),
            make_f2fs: dir.join(format!("make_f2fs{exe}")),
            make_f2fs_casefold: dir.join(format!("make_f2fs_casefold{exe}")),
            mke2fs_conf: dir.join("mke2fs.conf"),
            dir,
        }
    }

    /// Check that all required tool binaries and config files exist on disk.
    ///
    /// # Errors
    ///
    /// Returns an error if any required binary or config file is missing.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.mke2fs.is_file(),
            "missing mke2fs at {}",
            self.mke2fs.display()
        );
        anyhow::ensure!(
            self.make_f2fs.is_file(),
            "missing make_f2fs at {}",
            self.make_f2fs.display()
        );
        anyhow::ensure!(
            self.mke2fs_conf.is_file(),
            "missing mke2fs.conf at {}",
            self.mke2fs_conf.display()
        );
        Ok(())
    }

    /// Set the working directory and any platform-specific environment
    /// variables (e.g. `LD_LIBRARY_PATH` on Linux, `PATH` on Windows) so
    /// that the tool binary can find its dependencies.
    pub fn apply_runtime_env(&self, cmd: &mut Command) -> anyhow::Result<()> {
        cmd.current_dir(&self.dir);

        #[cfg(target_os = "linux")]
        {
            let lib_dir = self.dir.join("lib64");
            if lib_dir.is_dir() {
                if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
                    let mut paths = vec![lib_dir];
                    paths.extend(std::env::split_paths(&existing));
                    cmd.env("LD_LIBRARY_PATH", std::env::join_paths(paths)?);
                } else {
                    cmd.env("LD_LIBRARY_PATH", lib_dir);
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;

            let old_path = std::env::var_os("PATH").unwrap_or_default();
            let mut paths = vec![self.dir.clone()];
            paths.extend(std::env::split_paths(&old_path));
            cmd.env("PATH", std::env::join_paths(paths)?);
            cmd.creation_flags(0x0800_0000);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::FormatTools;
    use std::path::Path;

    #[test]
    fn format_tools_should_build_linux_layout() {
        let tools = FormatTools::from_platform_root(Path::new("/tmp/format-bin"), "linux");

        assert_eq!(tools.dir, Path::new("/tmp/format-bin/linux"));
        assert_eq!(tools.mke2fs, Path::new("/tmp/format-bin/linux/mke2fs"));
        assert_eq!(
            tools.make_f2fs,
            Path::new("/tmp/format-bin/linux/make_f2fs")
        );
        assert_eq!(
            tools.make_f2fs_casefold,
            Path::new("/tmp/format-bin/linux/make_f2fs_casefold")
        );
    }

    #[test]
    fn format_tools_should_build_windows_layout() {
        let tools = FormatTools::from_platform_root(Path::new("C:/format-bin"), "windows");

        assert_eq!(tools.dir, Path::new("C:/format-bin/windows"));
        assert_eq!(tools.mke2fs, Path::new("C:/format-bin/windows/mke2fs.exe"));
        assert_eq!(
            tools.make_f2fs,
            Path::new("C:/format-bin/windows/make_f2fs.exe")
        );
        assert_eq!(
            tools.make_f2fs_casefold,
            Path::new("C:/format-bin/windows/make_f2fs_casefold.exe")
        );
    }
}
