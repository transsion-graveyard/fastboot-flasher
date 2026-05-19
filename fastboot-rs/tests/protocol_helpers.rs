#![allow(missing_docs)]

use fastboot_rs::{
    parse_max_download_size,
    protocol::{FastBootCommand, FastBootResponse},
};

#[test]
fn command_format_should_match_fastboot_wire_strings() {
    assert_eq!(
        FastBootCommand::<&str>::Download(0x1000).to_string(),
        "download:00001000"
    );
    assert_eq!(FastBootCommand::Flash("boot_a").to_string(), "flash:boot_a");
    assert_eq!(FastBootCommand::SetActive("a").to_string(), "set_active:a");
    assert_eq!(
        FastBootCommand::<&str>::FlashingUnlock.to_string(),
        "flashing unlock"
    );
    assert_eq!(
        FastBootCommand::<&str>::FlashingLock.to_string(),
        "flashing lock"
    );
    assert_eq!(
        FastBootCommand::<&str>::Verify(4096).to_string(),
        "verify:4096"
    );
}

#[test]
fn response_parse_should_trim_after_null_byte() {
    let response = FastBootResponse::from_bytes(b"OKAYdone\0ignored").unwrap();

    assert_eq!(response, FastBootResponse::Okay("done".to_string()));
}

#[test]
fn max_download_size_should_accept_decimal_and_hex_values() {
    assert_eq!(parse_max_download_size("4096").unwrap(), 4096);
    assert_eq!(parse_max_download_size("0x1000").unwrap(), 4096);
}
