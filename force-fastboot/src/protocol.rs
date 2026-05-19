//!
//! Fastboot handshake protocol over a serial connection.
//!
//! Defines the [`SerialIo`] trait for abstracted serial I/O and the [`force_fastboot`]
//! function that implements the preloader handshake.

use std::io;

/// Abstract serial I/O operations used by the fastboot handshake protocol.
pub trait SerialIo {
    /// Read a single byte from the serial port.
    /// Returns `Ok(None)` on timeout or end-of-stream.
    fn read_byte(&mut self) -> io::Result<Option<u8>>;
    /// Discard any buffered input data.
    fn flush_input(&mut self) -> io::Result<()>;
    /// Write all bytes in `buf` to the serial port and flush the output.
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()>;
}

/// Execute the MTK preloader fastboot handshake: read bytes until `0x59` (`'Y'`) is received,
/// then flush the input buffer and send the `"FASTBOOT"` command.
pub fn force_fastboot(port: &mut dyn SerialIo) -> io::Result<()> {
    loop {
        match port.read_byte()? {
            Some(b'Y') => {
                port.flush_input()?;
                port.write_all(b"FASTBOOT")?;
                return Ok(());
            }
            _ => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeSerial {
        reads: Vec<u8>,
        writes: Vec<u8>,
        flushed: bool,
    }

    impl FakeSerial {
        fn new(reads: Vec<u8>) -> Self {
            Self {
                reads: reads.into_iter().rev().collect(),
                writes: Vec::new(),
                flushed: false,
            }
        }
    }

    impl SerialIo for FakeSerial {
        fn read_byte(&mut self) -> io::Result<Option<u8>> {
            Ok(self.reads.pop())
        }

        fn flush_input(&mut self) -> io::Result<()> {
            self.flushed = true;
            Ok(())
        }

        fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
            self.writes.extend_from_slice(buf);
            Ok(())
        }
    }

    #[test]
    fn handshake_sends_only_fastboot_after_start_byte() {
        let mut port = FakeSerial::new(vec![0x00, b'Y']);

        force_fastboot(&mut port).unwrap();

        assert!(port.flushed);
        assert_eq!(port.writes, b"FASTBOOT");
    }
}
