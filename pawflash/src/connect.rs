//! Poll `open_fastboot` every 250ms until a device is found.

use std::time::Duration;

use fastboot_rs::{open_fastboot_with_observer, FastbootDevice};
use tokio::time::sleep;

/// Shared retry delay used when reconnecting to a fastboot device.
pub const FASTBOOT_RETRY_DELAY_MS: u64 = 250;

/// Return the shared reconnect delay as a `Duration`.
pub fn fastboot_connect_retry_delay() -> Duration {
    Duration::from_millis(FASTBOOT_RETRY_DELAY_MS)
}

/// Poll `open_fastboot` every 250ms until a device is found.
pub async fn connect_fastboot() -> anyhow::Result<FastbootDevice> {
    loop {
        match try_connect_fastboot().await {
            Ok(dev) => return Ok(dev),
            Err(_) => {
                sleep(fastboot_connect_retry_delay()).await;
            }
        }
    }
}

/// Open a fastboot device once using nusb.
pub async fn try_connect_fastboot() -> anyhow::Result<FastbootDevice> {
    open_fastboot_with_observer(|_| {})
        .await
        .map_err(anyhow::Error::from)
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
            || async { Err(anyhow!("not found")) },
            std::time::Duration::from_millis(1),
            std::time::Duration::ZERO,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("timed out"));
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
