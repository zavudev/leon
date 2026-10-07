//! Versioned, length-prefixed frames.
//!
//! ```text
//! +----------------+---------+--------------------------+
//! | length: u32 BE | version | payload: postcard message |
//! +----------------+---------+--------------------------+
//! ```
//!
//! `length` counts the payload only. A frame whose length exceeds
//! [`MAX_FRAME_LEN`] is refused as soon as its header is seen, so a hostile
//! peer cannot make a decoder buffer more than that plus the header.

use thiserror::Error;

use crate::message::{Message, PROTOCOL_VERSION};

/// The largest payload of one frame: a little over two maximal command
/// outputs.
pub const MAX_FRAME_LEN: usize = 10 * 1024 * 1024;
/// The size of the header.
pub const HEADER_LEN: usize = 5;

/// Why a frame could not be read or written.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum FrameError {
    /// More bytes are needed.
    #[error("the frame is incomplete")]
    Incomplete,
    /// The header announces more than [`MAX_FRAME_LEN`].
    #[error("the frame is larger than the limit ({0} bytes)")]
    TooLarge(usize),
    /// The peer speaks another protocol version.
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u8),
    /// The payload is not a valid message.
    #[error("malformed message: {0}")]
    Malformed(String),
    /// The message breaks a size limit.
    #[error("message over a limit: {0}")]
    OverLimit(&'static str),
}

/// Encodes `message` as one frame.
pub fn encode_frame(message: &Message) -> Result<Vec<u8>, FrameError> {
    message.validate()?;
    let payload =
        postcard::to_allocvec(message).map_err(|error| FrameError::Malformed(error.to_string()))?;
    if payload.len() > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(payload.len()));
    }
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.push(PROTOCOL_VERSION as u8);
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Decodes the frame at the start of `bytes`, returning the message and how
/// many bytes it used.
pub fn decode_frame(bytes: &[u8]) -> Result<(Message, usize), FrameError> {
    let header = bytes.get(..HEADER_LEN).ok_or(FrameError::Incomplete)?;
    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    if length > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(length));
    }
    if header[4] != PROTOCOL_VERSION as u8 {
        return Err(FrameError::UnsupportedVersion(header[4]));
    }
    let payload = bytes
        .get(HEADER_LEN..HEADER_LEN + length)
        .ok_or(FrameError::Incomplete)?;
    let message: Message =
        postcard::from_bytes(payload).map_err(|error| FrameError::Malformed(error.to_string()))?;
    message.validate()?;
    Ok((message, HEADER_LEN + length))
}

/// Reads frames from a byte stream that arrives in arbitrary pieces.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
    failed: Option<FrameError>,
}

impl FrameDecoder {
    /// An empty decoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds received bytes. After an error the decoder stays failed: the
    /// stream cannot be resynchronised and the connection should be closed.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.failed.is_none() {
            self.buffer.extend_from_slice(bytes);
        }
    }

    /// The next complete message, if there is one.
    pub fn next_message(&mut self) -> Result<Option<Message>, FrameError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        match decode_frame(&self.buffer) {
            Ok((message, used)) => {
                self.buffer.drain(..used);
                Ok(Some(message))
            }
            Err(FrameError::Incomplete) => Ok(None),
            Err(error) => {
                self.failed = Some(error.clone());
                self.buffer = Vec::new();
                Err(error)
            }
        }
    }

    /// How many bytes are waiting for the rest of their frame.
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;

    fn sample() -> Message {
        Message::Ping { nonce: 42 }
    }

    #[test]
    fn a_frame_round_trips_and_reports_its_length() {
        let bytes = encode_frame(&sample()).unwrap();
        let (message, used) = decode_frame(&bytes).unwrap();
        assert_eq!(message, sample());
        assert_eq!(used, bytes.len());
    }

    #[test]
    fn every_truncation_of_a_frame_is_incomplete_not_a_panic() {
        let bytes = encode_frame(&Message::Pong { nonce: 7 }).unwrap();
        for cut in 0..bytes.len() {
            assert_eq!(
                decode_frame(&bytes[..cut]),
                Err(FrameError::Incomplete),
                "{cut}"
            );
        }
    }

    #[test]
    fn an_oversized_header_is_refused_before_any_payload_is_read() {
        let mut bytes = ((MAX_FRAME_LEN + 1) as u32).to_be_bytes().to_vec();
        bytes.push(PROTOCOL_VERSION as u8);
        assert_eq!(
            decode_frame(&bytes),
            Err(FrameError::TooLarge(MAX_FRAME_LEN + 1))
        );
    }

    #[test]
    fn another_protocol_version_is_refused() {
        let mut bytes = encode_frame(&sample()).unwrap();
        bytes[4] = 99;
        assert_eq!(
            decode_frame(&bytes),
            Err(FrameError::UnsupportedVersion(99))
        );
    }

    #[test]
    fn two_frames_back_to_back_decode_one_at_a_time() {
        let mut bytes = encode_frame(&sample()).unwrap();
        bytes.extend(encode_frame(&Message::Pong { nonce: 1 }).unwrap());
        let (first, used) = decode_frame(&bytes).unwrap();
        assert_eq!(first, sample());
        let (second, _) = decode_frame(&bytes[used..]).unwrap();
        assert_eq!(second, Message::Pong { nonce: 1 });
    }

    #[test]
    fn the_streaming_decoder_copes_with_one_byte_at_a_time() {
        let mut stream = encode_frame(&sample()).unwrap();
        stream.extend(encode_frame(&Message::Pong { nonce: 5 }).unwrap());
        let mut decoder = FrameDecoder::new();
        let mut got = Vec::new();
        for byte in stream {
            decoder.push(&[byte]);
            while let Some(message) = decoder.next_message().unwrap() {
                got.push(message);
            }
        }
        assert_eq!(got, vec![sample(), Message::Pong { nonce: 5 }]);
        assert_eq!(decoder.buffered(), 0);
    }

    #[test]
    fn the_streaming_decoder_stays_failed_after_garbage_and_buffers_nothing_more() {
        let mut decoder = FrameDecoder::new();
        decoder.push(&[0xFF; 5]);
        assert!(matches!(
            decoder.next_message(),
            Err(FrameError::TooLarge(_))
        ));
        decoder.push(&[0; 1000]);
        assert!(decoder.next_message().is_err());
        assert_eq!(decoder.buffered(), 0);
    }

    #[test]
    fn a_payload_that_is_not_a_message_is_malformed() {
        let mut bytes = vec![0, 0, 0, 3, PROTOCOL_VERSION as u8, 0xEE, 0xEE, 0xEE];
        assert!(matches!(
            decode_frame(&bytes),
            Err(FrameError::Malformed(_))
        ));
        bytes.truncate(5);
        assert_eq!(decode_frame(&bytes), Err(FrameError::Incomplete));
    }

    /// A tiny deterministic generator, so the randomised tests are repeatable.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
    }

    #[test]
    fn random_garbage_never_panics_and_never_decodes_past_its_end() {
        let mut rng = Lcg(0x1eed);
        for _ in 0..4000 {
            let len = (rng.next() % 200) as usize;
            let mut bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            if len >= 5 && rng.next() % 2 == 0 {
                // Make the header plausible so the payload decoder is reached.
                let payload = (len - 5) as u32;
                bytes[..4].copy_from_slice(&payload.to_be_bytes());
                bytes[4] = PROTOCOL_VERSION as u8;
            }
            if let Ok((_, used)) = decode_frame(&bytes) {
                assert!(used <= bytes.len());
            }
        }
    }

    #[test]
    fn flipping_any_single_byte_of_a_valid_frame_never_panics() {
        let message = Message::Hello {
            protocol: PROTOCOL_VERSION,
            app_version: "0.1.0".into(),
            device_name: "laptop".into(),
            token: Some(vec![1, 2, 3]),
        };
        let bytes = encode_frame(&message).unwrap();
        for at in 0..bytes.len() {
            for flip in [0x01u8, 0x80, 0xFF] {
                let mut copy = bytes.clone();
                copy[at] ^= flip;
                let _ = decode_frame(&copy);
            }
        }
    }
}
