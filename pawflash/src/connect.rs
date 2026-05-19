//! Poll `open_fastboot` every 250ms until a device is found.

use std::time::Duration;

use fastboot_rs::{open_fastboot_with_preferred_backend, BackendKind, FastbootDevice};
use tokio::time::sleep;

#[cfg(test)]
mod tests {
    #[test]
    fn fastboot_connect_retry_delay_matches_shared_policy() {
        assert_eq!(
            super::fastboot_connect_retry_delay(),
            std::time::Duration::from_millis(250)
        );
    }
}

/// Shared retry delay used when reconnecting to a fastboot device.
pub const FASTBOOT_RETRY_DELAY_MS: u64 = 250;

/// Return the shared reconnect delay as a `Duration`.
pub fn fastboot_connect_retry_delay() -> Duration {
    Duration::from_millis(FASTBOOT_RETRY_DELAY_MS)
}

/// Poll `open_fastboot` every 250ms until a device is found.
pub async fn connect_fastboot() -> anyhow::Result<FastbootDevice> {
    loop {
        match try_connect_fastboot_prefer_backend(None).await {
            Ok(dev) => return Ok(dev),
            Err(_) => {
                sleep(fastboot_connect_retry_delay()).await;
            }
        }
    }
}

/// Open a fastboot device once, trying the preferred backend first when set.
pub async fn try_connect_fastboot_prefer_backend(
    preferred_backend: Option<BackendKind>,
) -> anyhow::Result<FastbootDevice> {
    open_fastboot_with_preferred_backend(preferred_backend, |_| {})
        .await
        .map_err(anyhow::Error::from)
}
