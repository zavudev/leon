//! A bounded buffer of recent terminal output with absolute byte offsets.
//!
//! Offsets count every byte the terminal ever produced, starting at zero. The
//! ring keeps the newest `capacity` bytes. A client that has seen everything
//! up to offset `n` asks for the rest with [`Ring::read_from`]: it gets each
//! later byte exactly once, or is told that the ring no longer reaches back
//! to `n` (a gap) and starts from the oldest byte kept.

use std::collections::VecDeque;

/// What a re-attaching client is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    /// The offset of the first byte of `bytes`.
    pub from: u64,
    /// Whether the client asked for bytes that are no longer kept.
    pub gap: bool,
    /// The bytes from `from` to the end.
    pub bytes: Vec<u8>,
}

/// The ring.
#[derive(Debug)]
pub struct Ring {
    buffer: VecDeque<u8>,
    start: u64,
    capacity: usize,
}

impl Ring {
    /// A ring that keeps at most `capacity` bytes.
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: VecDeque::new(),
            start: 0,
            capacity: capacity.max(1),
        }
    }

    /// The offset of the oldest byte kept.
    pub fn first_offset(&self) -> u64 {
        self.start
    }

    /// The offset one past the newest byte.
    pub fn end_offset(&self) -> u64 {
        self.start + self.buffer.len() as u64
    }

    /// Appends output and returns the offset of its first byte.
    pub fn append(&mut self, bytes: &[u8]) -> u64 {
        let offset = self.end_offset();
        self.buffer.extend(bytes);
        let excess = self.buffer.len().saturating_sub(self.capacity);
        if excess > 0 {
            self.buffer.drain(..excess);
            self.start += excess as u64;
        }
        offset
    }

    /// Everything from `offset` on.
    pub fn read_from(&self, offset: u64) -> Replay {
        let end = self.end_offset();
        let (from, gap) = if offset < self.start {
            (self.start, true)
        } else {
            // A client cannot have seen bytes that do not exist yet.
            (offset.min(end), false)
        };
        let skip = (from - self.start) as usize;
        let bytes = self.buffer.iter().skip(skip).copied().collect();
        Replay { from, gap, bytes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appending_returns_the_offset_of_the_first_new_byte() {
        let mut ring = Ring::new(100);
        assert_eq!(ring.append(b"abc"), 0);
        assert_eq!(ring.append(b"de"), 3);
        assert_eq!(ring.end_offset(), 5);
    }

    #[test]
    fn reading_from_an_offset_returns_exactly_the_rest() {
        let mut ring = Ring::new(100);
        ring.append(b"hello world");
        let replay = ring.read_from(6);
        assert_eq!(
            replay,
            Replay {
                from: 6,
                gap: false,
                bytes: b"world".to_vec()
            }
        );
        assert_eq!(ring.read_from(11).bytes, b"");
        assert_eq!(ring.read_from(0).bytes, b"hello world");
    }

    #[test]
    fn the_oldest_bytes_are_dropped_at_capacity_and_offsets_stay_absolute() {
        let mut ring = Ring::new(4);
        ring.append(b"abcdef");
        assert_eq!(ring.first_offset(), 2);
        assert_eq!(ring.end_offset(), 6);
        assert_eq!(ring.read_from(2).bytes, b"cdef");
    }

    #[test]
    fn asking_for_bytes_that_are_gone_is_reported_as_a_gap() {
        let mut ring = Ring::new(4);
        ring.append(b"abcdefgh");
        let replay = ring.read_from(1);
        assert!(replay.gap);
        assert_eq!(replay.from, 4);
        assert_eq!(replay.bytes, b"efgh");
    }

    #[test]
    fn asking_beyond_the_end_is_clamped_not_a_panic() {
        let mut ring = Ring::new(10);
        ring.append(b"abc");
        let replay = ring.read_from(999);
        assert_eq!(
            replay,
            Replay {
                from: 3,
                gap: false,
                bytes: Vec::new()
            }
        );
    }

    #[test]
    fn resuming_in_pieces_never_loses_or_repeats_a_byte() {
        let mut ring = Ring::new(64);
        let mut seen = Vec::new();
        let mut cursor = 0u64;
        for chunk in 0..50u8 {
            ring.append(&[chunk; 7]);
            if chunk % 3 == 0 {
                let replay = ring.read_from(cursor);
                assert!(!replay.gap);
                assert_eq!(replay.from, cursor);
                cursor += replay.bytes.len() as u64;
                seen.extend(replay.bytes);
            }
        }
        let replay = ring.read_from(cursor);
        seen.extend(replay.bytes);
        let expected: Vec<u8> = (0..50u8).flat_map(|c| [c; 7]).collect();
        assert_eq!(seen, expected);
    }
}
