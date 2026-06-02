//! Poll `open_fastboot` every 250ms until a device is found.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use fastboot_rs::{open_fastboot_with_observer, FastbootDevice};
use tokio::time::sleep;

/// Shared retry delay used when reconnecting to a fastboot device.
pub const FASTBOOT_RETRY_DELAY_MS: u64 = 250;
const FASTBOOT_CONNECT_TIMEOUT_SECS: u64 = 120;

/// Return the shared reconnect delay as a `Duration`.
pub fn fastboot_connect_retry_delay() -> Duration {
    Duration::from_millis(FASTBOOT_RETRY_DELAY_MS)
}

/// Poll `open_fastboot` every 250ms until a device is found.
pub async fn connect_fastboot() -> anyhow::Result<FastbootDevice> {
    connect_fastboot_with_timeout_and_retry(
        try_connect_fastboot,
        Some(Duration::from_secs(FASTBOOT_CONNECT_TIMEOUT_SECS)),
        fastboot_connect_retry_delay(),
        None,
    )
    .await
}

/// Poll `open_fastboot` indefinitely until a device is found or cancellation is requested.
pub async fn connect_fastboot_until_cancelled(
    cancel_requested: Option<&AtomicBool>,
) -> anyhow::Result<FastbootDevice> {
    connect_fastboot_with_timeout_and_retry(
        try_connect_fastboot,
        None,
        fastboot_connect_retry_delay(),
        cancel_requested,
    )
    .await
}

/// Poll `open_fastboot` until a device is found or the provided timeout expires.
pub async fn connect_fastboot_with_timeout(timeout: Duration) -> anyhow::Result<FastbootDevice> {
    connect_fastboot_with_timeout_and_retry(
        try_connect_fastboot,
        Some(timeout),
        fastboot_connect_retry_delay(),
        None,
    )
    .await
}

/// Open a fastboot device once using nusb.
pub async fn try_connect_fastboot() -> anyhow::Result<FastbootDevice> {
    open_fastboot_with_observer(|_| {})
        .await
        .map_err(anyhow::Error::from)
}

async fn connect_fastboot_with_timeout_and_retry<T, Attempt, Fut>(
    mut attempt_connect: Attempt,
    timeout: Option<Duration>,
    retry_delay: Duration,
    cancel_requested: Option<&AtomicBool>,
) -> anyhow::Result<T>
where
    Attempt: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let deadline = timeout.map(|timeout| std::time::Instant::now() + timeout);
    let mut attempts = 0_u64;

    loop {
        if cancel_requested.is_some_and(|cancel| cancel.load(Ordering::SeqCst)) {
            anyhow::bail!("cancelled by user");
        }

        attempts = attempts.saturating_add(1);
        match attempt_connect().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
                    let attempts_label = if attempts == 1 { "attempt" } else { "attempts" };
                    return Err(anyhow::anyhow!(
                        "timed out after {:?} waiting for fastboot device after {} {}",
                        timeout.expect("timeout is present when deadline is set"),
                        attempts,
                        attempts_label
                    )
                    .context(format!("last probe error: {error}")));
                }
            }
        }

        if retry_delay.is_zero() {
            continue;
        }

        sleep(retry_delay).await;
    }
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    #[test]
    fn fastboot_connect_retry_delay_matches_shared_policy() {
        assert_eq!(
            super::fastboot_connect_retry_delay(),
            std::time::Duration::from_millis(250)
        );
    }

    #[tokio::test]
    async fn connect_fastboot_times_out_when_device_never_appears() {
        let error = super::connect_fastboot_with_timeout_and_retry(
            || async { Err::<(), anyhow::Error>(anyhow!("not found")) },
            Some(std::time::Duration::from_millis(1)),
            std::time::Duration::ZERO,
            None,
        )
        .await
        .unwrap_err();

        let rendered = format!("{error:#}");
        assert!(rendered.contains("timed out"));
        assert!(rendered.contains("last probe error: not found"));
    }

    #[tokio::test]
    async fn connect_fastboot_retries_until_device_is_found() {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let result = super::connect_fastboot_with_timeout_and_retry(
            {
                let attempts = attempts.clone();
                move || {
                    let attempts = attempts.clone();
                    async move {
                        let attempt = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if attempt < 2 {
                            Err(anyhow!("not found"))
                        } else {
                            Ok(())
                        }
                    }
                }
            },
            Some(std::time::Duration::from_millis(10)),
            std::time::Duration::ZERO,
            None,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn connect_fastboot_without_timeout_stops_when_cancelled() {
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancel_for_task = cancelled.clone();

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            cancel_for_task.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        let error = super::connect_fastboot_with_timeout_and_retry(
            || async { Err::<(), anyhow::Error>(anyhow!("not found")) },
            None,
            std::time::Duration::from_millis(50),
            Some(cancelled.as_ref()),
        )
        .await
        .unwrap_err();

        assert_eq!(error.to_string(), "cancelled by user");
    }
}
