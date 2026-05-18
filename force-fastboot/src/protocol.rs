use std::io;

pub trait SerialIo {
    fn read_byte(&mut self) -> io::Result<Option<u8>>;
    fn flush_input(&mut self) -> io::Result<()>;
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()>;
}

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
