use terminal_output::chrome::{notice_box, Tone};

#[cfg(any(target_os = "linux", test))]
const RULE_PATH: &str = "/etc/udev/rules.d/99-mediatek-preloader.rules";
#[cfg(any(target_os = "linux", test))]
const MEDIA_TEK_UDEV_RULES: &str = r#"# MediaTek Preloader / BROM / Download Agent
# Common IDs: 0e8d:2000 preloader, 0e8d:0003 DA/BROM

SUBSYSTEM=="usb", ATTR{idVendor}=="0e8d", MODE="0666", TAG+="uaccess"
SUBSYSTEM=="tty", ATTRS{idVendor}=="0e8d", MODE="0666", TAG+="uaccess"
"#;

#[cfg(any(target_os = "linux", test))]
pub fn render_udev_rules() -> &'static str {
    MEDIA_TEK_UDEV_RULES
}

#[cfg(target_os = "linux")]
pub fn auto_install_linux_rule(_candidate: &crate::serial::PortCandidate) -> bool {
    eprintln!(
        "{}",
        notice_box(
            Tone::Warning,
            "udev install",
            &format!(
                "Normal user cannot open the preloader serial port.\nInstalling Linux udev rules with sudo at {RULE_PATH}"
            )
        )
    );

    let existing = read_existing_rule();

    match install_udev_rules(existing.as_deref()) {
        Ok(true) => {
            eprintln!("udev rule installed at {RULE_PATH}");
            eprintln!("udev rules reloaded and triggered.");
        }
        Ok(false) => {
            eprintln!("udev rule already exists at {RULE_PATH}");
        }
        Err(e) => {
            eprintln!("Failed to install udev rule: {e}");
            return false;
        }
    }

    eprintln!(
        "{}",
        notice_box(
            Tone::Info,
            "reconnect device",
            "Reconnect the device if the preloader port still has old permissions."
        )
    );
    true
}

#[cfg(not(target_os = "linux"))]
pub fn auto_install_linux_rule(candidate: &crate::serial::PortCandidate) -> bool {
    eprintln!(
        "{}",
        notice_box(
            Tone::Warning,
            "permission denied",
            &format!(
                "Permission denied opening {}\nAutomatic udev setup is only supported on Linux.\nUse --port to specify a device, or fix permissions manually for your OS.",
                candidate.device
            )
        )
    );
    false
}

#[cfg(target_os = "linux")]
pub fn print_permission_guidance(candidate: &crate::serial::PortCandidate) {
    eprintln!(
        "{}",
        notice_box(
            Tone::Warning,
            "permission denied",
            &format!(
                "Permission denied opening {}\nAuto udev setup is disabled. Install these rules manually:\nsudo tee {RULE_PATH} >/dev/null <<'EOF'\n{}EOF\nsudo udevadm control --reload-rules\nsudo udevadm trigger",
                candidate.device,
                MEDIA_TEK_UDEV_RULES
            )
        )
    );
}

#[cfg(not(target_os = "linux"))]
pub fn print_permission_guidance(candidate: &crate::serial::PortCandidate) {
    eprintln!(
        "{}",
        notice_box(
            Tone::Warning,
            "permission denied",
            &format!(
                "Permission denied opening {}\nAutomatic udev setup is only supported on Linux.\nUse --port to specify a device, or fix permissions manually for your OS.",
                candidate.device
            )
        )
    );
}

#[cfg(target_os = "linux")]
fn read_existing_rule() -> Option<String> {
    std::fs::read_to_string(RULE_PATH).ok()
}

#[cfg(target_os = "linux")]
fn install_udev_rules(existing_content: Option<&str>) -> anyhow::Result<bool> {
    let rules = render_udev_rules();

    if existing_content == Some(rules) {
        return Ok(false);
    }

    std::process::Command::new("sudo")
        .args(["tee", RULE_PATH])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(rules.as_bytes())?;
            }
            child.wait()?;
            Ok(())
        })?;

    std::process::Command::new("sudo")
        .args(["udevadm", "control", "--reload-rules"])
        .status()?;

    std::process::Command::new("sudo")
        .args(["udevadm", "trigger"])
        .status()?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_udev_rules_includes_mediatek_ids() {
        let rules = render_udev_rules();

        assert!(rules.contains("MediaTek Preloader / BROM / Download Agent"));
        assert!(rules.contains(r#"SUBSYSTEM=="usb""#));
        assert!(rules.contains(r#"ATTR{idVendor}=="0e8d""#));
        assert!(rules.contains(r#"SUBSYSTEM=="tty""#));
        assert!(rules.contains(r#"ATTRS{idVendor}=="0e8d""#));
        assert!(rules.contains(r#"MODE="0666""#));
        assert!(rules.contains(r#"TAG+="uaccess""#));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn install_udev_rule_skips_when_content_matches() {
        let result = install_udev_rules(Some(render_udev_rules()));
        assert!(!result.unwrap());
    }
}
