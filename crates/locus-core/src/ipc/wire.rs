//! Newline-delimited message framing shared by the daemon and its clients.

use std::io::{self, BufRead, Write};

use crate::ipc::protocol::MAX_MESSAGE_BYTES;

/// Outcome of attempting to read a single framed message.
#[derive(Debug)]
pub enum ReadOutcome {
    /// A complete message (without its trailing newline).
    Message(Vec<u8>),
    /// The message exceeded the configured size limit.
    TooLarge,
    /// The stream reached end-of-file with no pending data.
    Eof,
}

/// Writes a single JSON value as a newline-delimited message.
pub fn write_message<W: Write>(writer: &mut W, bytes: &[u8]) -> io::Result<()> {
    writer.write_all(bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// Reads a single newline-delimited message, bounding memory use at `max`
/// bytes. Oversized messages are reported as [`ReadOutcome::TooLarge`] without
/// buffering the remainder.
pub fn read_message<R: BufRead>(reader: &mut R, max: usize) -> io::Result<ReadOutcome> {
    let mut buf = Vec::new();
    loop {
        let available = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };

        if available.is_empty() {
            if buf.is_empty() {
                return Ok(ReadOutcome::Eof);
            }
            return Ok(ReadOutcome::Message(buf));
        }

        if let Some(pos) = available.iter().position(|byte| *byte == b'\n') {
            if buf.len() + pos > max {
                reader.consume(pos + 1);
                return Ok(ReadOutcome::TooLarge);
            }
            buf.extend_from_slice(&available[..pos]);
            reader.consume(pos + 1);
            return Ok(ReadOutcome::Message(buf));
        }

        if buf.len() + available.len() > max {
            let len = available.len();
            reader.consume(len);
            return Ok(ReadOutcome::TooLarge);
        }

        buf.extend_from_slice(available);
        let len = available.len();
        reader.consume(len);
    }
}

/// Default maximum message size for callers that don't override it.
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = MAX_MESSAGE_BYTES;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    #[test]
    fn reads_two_messages() {
        let data = b"hello\nworld\n";
        let mut reader = BufReader::new(&data[..]);
        match read_message(&mut reader, 1024).unwrap() {
            ReadOutcome::Message(m) => assert_eq!(m, b"hello"),
            other => panic!("unexpected {other:?}"),
        }
        match read_message(&mut reader, 1024).unwrap() {
            ReadOutcome::Message(m) => assert_eq!(m, b"world"),
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(
            read_message(&mut reader, 1024).unwrap(),
            ReadOutcome::Eof
        ));
    }

    #[test]
    fn oversized_message_is_flagged() {
        let data = b"aaaaaaaaaa\n";
        let mut reader = BufReader::new(&data[..]);
        assert!(matches!(
            read_message(&mut reader, 4).unwrap(),
            ReadOutcome::TooLarge
        ));
    }
}
