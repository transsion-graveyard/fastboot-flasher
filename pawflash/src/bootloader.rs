//! Shared bootloader transition helpers used by the CLI and GUI adapters.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;

use crate::connect::{connect_fastboot_until_cancelled, connect_fastboot_with_timeout};
use crate::device::reboot_device_bootloader;
use crate::{FastbootDevice, FlashRunControl, ForceFastbootError, ForceFastbootOptions};

const FASTBOOT_DETECTION_GRACE: Duration = Duration::from_secs(5);

/// Progress stages emitted while forcing preloader into fastboot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForceFastbootStage {
    /// Waiting for a MediaTek preloader serial port to appear.
    WaitingForPreloader,
    /// Waiting for a fastboot USB interface after sending `FASTBOOT`.
    WaitingForFastboot,
    /// The previous attempt did not yield a fastboot device; retrying.
    Retrying,
    /// A fastboot-capable interface has been detected.
    Detected,
}

/// Return the grace window used between a successful serial force-fastboot attempt
/// and the next retry cycle.
pub fn fastboot_detection_grace_period() -> Duration {
    FASTBOOT_DETECTION_GRACE
}

/// Keep forcing preloader into fastboot until a fastboot interface is detected.
pub async fn force_fastboot_until_detected_with_progress<F>(
    options: &ForceFastbootOptions,
    control: Option<&FlashRunControl>,
    report: F,
) -> anyhow::Result<FastbootDevice>
where
    F: FnMut(ForceFastbootStage),
{
    let options = options.clone();
    let cancel_requested = shared_cancel_flag(control);

    force_fastboot_until_detected_inner(
        || {
            let options = options.clone();
            let cancel_requested = cancel_requested.clone();
            async move {
                tokio::task::spawn_blocking(move || {
                    force_fastboot::run_force_fastboot_quiet_cancellable(
                        &options,
                        cancel_requested.as_ref(),
                    )
                })
                .await
                .context("join force-fastboot worker")?
                .map_err(anyhow::Error::from)
            }
        },
        |timeout| async move { connect_fastboot_with_timeout(timeout).await },
        control,
        report,
    )
    .await
}

/// Keep forcing preloader into fastboot until a fastboot interface is detected.
pub async fn force_fastboot_until_detected(
    options: &ForceFastbootOptions,
    control: Option<&FlashRunControl>,
) -> anyhow::Result<FastbootDevice> {
    force_fastboot_until_detected_with_progress(options, control, |_| {}).await
}

/// Reboot an already-connected fastboot device into bootloader mode and wait until
/// a fastboot interface reappears.
pub async fn reboot_device_bootloader_until_detected(
    dev: &mut FastbootDevice,
    control: Option<&FlashRunControl>,
) -> anyhow::Result<FastbootDevice> {
    ensure_not_cancelled(control)?;
    reboot_device_bootloader(dev).await?;
    connect_fastboot_until_cancelled(control.map(|control| control.cancel_requested.as_ref())).await
}

async fn force_fastboot_until_detected_inner<
    T,
    Attempt,
    AttemptFuture,
    Probe,
    ProbeFuture,
    Report,
>(
    mut force_attempt: Attempt,
    mut probe_fastboot: Probe,
    control: Option<&FlashRunControl>,
    mut report: Report,
) -> anyhow::Result<T>
where
    Attempt: FnMut() -> AttemptFuture,
    AttemptFuture: Future<Output = anyhow::Result<()>>,
    Probe: FnMut(Duration) -> ProbeFuture,
    ProbeFuture: Future<Output = anyhow::Result<T>>,
    Report: FnMut(ForceFastbootStage),
{
    loop {
        ensure_not_cancelled(control)?;
        report(ForceFastbootStage::WaitingForPreloader);

        match force_attempt().await {
            Ok(()) => {}
            Err(error) if is_retryable_force_fastboot_error(&error) => {
                ensure_not_cancelled(control)?;
                // The device may already be in fastboot (e.g. a previous serial
                // handshake succeeded but the fastboot probe timed out).  Check
                // before looping back to a serial attempt that will never appear.
                match probe_fastboot(FASTBOOT_DETECTION_GRACE).await {
                    Ok(device) => {
                        report(ForceFastbootStage::Detected);
                        return Ok(device);
                    }
                    Err(_) => {
                        report(ForceFastbootStage::Retrying);
                        continue;
                    }
                }
            }
            Err(error) => return Err(error),
        }

        ensure_not_cancelled(control)?;
        report(ForceFastbootStage::WaitingForFastboot);

        match probe_fastboot(FASTBOOT_DETECTION_GRACE).await {
            Ok(device) => {
                report(ForceFastbootStage::Detected);
                return Ok(device);
            }
            Err(error) => {
                ensure_not_cancelled(control)?;
                let _ = error;
                report(ForceFastbootStage::Retrying);
            }
        }
    }
}

fn shared_cancel_flag(control: Option<&FlashRunControl>) -> Arc<AtomicBool> {
    control.map_or_else(
        || Arc::new(AtomicBool::new(false)),
        |control| control.cancel_requested.clone(),
    )
}

fn ensure_not_cancelled(control: Option<&FlashRunControl>) -> anyhow::Result<()> {
    if control.is_some_and(|control| control.cancel_requested.load(Ordering::SeqCst)) {
        anyhow::bail!("cancelled by user");
    }

    Ok(())
}

fn is_retryable_force_fastboot_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ForceFastbootError>()
        .is_some_and(|error| {
            matches!(
                error,
                ForceFastbootError::NoDevice(_)
                    | ForceFastbootError::Protocol(_)
                    | ForceFastbootError::Serial(_)
            )
        })
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use std::sync::atomic::AtomicUsize;

    use super::*;

    #[tokio::test]
    async fn force_fastboot_retries_until_fastboot_is_detected() {
        let attempts = Arc::new(AtomicUsize::new(0));

        let result = force_fastboot_until_detected_inner(
            || async { Ok(()) },
            {
                let attempts = attempts.clone();
                move |_timeout| {
                    let attempts = attempts.clone();
                    async move {
                        let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                        if attempt == 0 {
                            Err(anyhow!("not detected yet"))
                        } else {
                            Ok("fastboot-ready")
                        }
                    }
                }
            },
            None,
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(result, "fastboot-ready");
        // force_attempt succeeds both times; probe fails once then succeeds.
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn force_fastboot_retries_on_retryable_force_errors() {
        let attempts = Arc::new(AtomicUsize::new(0));

        let result = force_fastboot_until_detected_inner(
            {
                let attempts = attempts.clone();
                move || {
                    let attempts = attempts.clone();
                    async move {
                        let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                        if attempt == 0 {
                            Err(anyhow!(ForceFastbootError::Protocol(
                                "timed out".to_string()
                            )))
                        } else {
                            Ok(())
                        }
                    }
                }
            },
            |_timeout| async { Ok("fastboot-ready") },
            None,
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(result, "fastboot-ready");
        // force_attempt failed once (attempt 0), but the fastboot probe
        // succeeded immediately, so only 1 serial attempt was needed.
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn force_fastboot_stops_when_cancelled() {
        let control = FlashRunControl::default();
        control.request_cancel();

        let error = force_fastboot_until_detected_inner(
            || async { Ok(()) },
            |_timeout| async { Ok::<_, anyhow::Error>("fastboot-ready") },
            Some(&control),
            |_| {},
        )
        .await
        .unwrap_err();

        assert_eq!(error.to_string(), "cancelled by user");
    }
}
