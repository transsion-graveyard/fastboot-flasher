//! Poll `open_fastboot` every 250ms until a device is found.

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
        Duration::from_secs(FASTBOOT_CONNECT_TIMEOUT_SECS),
        fastboot_connect_retry_delay(),
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
    timeout: Duration,
    retry_delay: Duration,
) -> anyhow::Result<T>
where
    Attempt: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let deadline = std::time::Instant::now() + timeout;
    let mut attempts = 0_u64;

    loop {
        attempts = attempts.saturating_add(1);
        match attempt_connect().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                if std::time::Instant::now() >= deadline {
                    let attempts_label = if attempts == 1 { "attempt" } else { "attempts" };
                    return Err(anyhow::anyhow!(
                        "timed out after {:?} waiting for fastboot device after {} {}",
                        timeout,
                        attempts,
                        attempts_label
                    )
                    .context(format!("last probe error: {error}")));
                }
            }
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
            std::time::Duration::from_millis(1),
            std::time::Duration::ZERO,
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
            std::time::Duration::from_millis(10),
            std::time::Duration::ZERO,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
    }
}
