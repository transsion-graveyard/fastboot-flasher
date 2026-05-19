//! Poll `open_fastboot` every 250ms until a device is found.

use std::time::Duration;

use fastboot_rs::{open_fastboot, FastbootDevice};
use tokio::time::sleep;

/// Poll `open_fastboot` every 250ms until a device is found.
pub async fn connect_fastboot() -> anyhow::Result<FastbootDevice> {
    loop {
        match open_fastboot().await {
            Ok(dev) => return Ok(dev),
            Err(_) => {
                sleep(Duration::from_millis(250)).await;
            }
        }
    }
}