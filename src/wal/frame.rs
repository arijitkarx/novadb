//! WAL frame encoding.
//!
//! A frame is the smallest durable unit of the write-ahead log:
//!
//! ```text
//! ┌───────┬─────────┬──────────┬──────────┬──────────┬──────────┐
//! │ magic │ version │  seq u64 │  len u32 │ crc32    │ payload  │
//! │ 1 B   │ 2 B     │  (LE)    │  (LE)    │ (LE)     │ len B    │
//! └───────┴─────────┴──────────┴──────────┴──────────┴──────────┘
//! ```
//!
//! The CRC covers the payload. Recovery treats a frame with a bad magic or a
//! trailing partial frame as a torn tail: everything before it is durable.

use serde::{Deserialize, Serialize};

use crate::error::{NovaError, Result};
use crate::storage::format::{crc32, PersistedRecord, WAL_FRAME_MAGIC, WAL_VERSION};

pub const WAL_FRAME_HEADER_LEN: usize = 1 + 2 + 8 + 4 + 4;

/// A durable mutation applied to a collection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WalOp {
    /// Create a collection with the given dimensionality. Every collection's
    /// WAL begins with this frame, so a collection that never reached a
    /// snapshot can still be reconstructed after a crash.
    Create(u32),
    /// Insert or replace a record (in its persisted, bincode-safe form).
    Put(PersistedRecord),
    /// Delete a record by id.
    Delete(u64),
}

/// One framed WAL entry.
#[derive(Debug, Clone, PartialEq)]
pub struct WalFrame {
    /// Monotonically increasing sequence number (0-based within a log file).
    pub seq: u64,
    pub op: WalOp,
}

/// Encode a frame to bytes.
pub fn encode_frame(frame: &WalFrame) -> Result<Vec<u8>> {
    let payload = bincode::serialize(&frame.op).map_err(NovaError::from)?;
    let mut buf = Vec::with_capacity(WAL_FRAME_HEADER_LEN + payload.len());
    buf.push(WAL_FRAME_MAGIC);
    buf.extend_from_slice(&WAL_VERSION.to_le_bytes());
    buf.extend_from_slice(&frame.seq.to_le_bytes());
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(&crc32(&payload).to_le_bytes());
    buf.extend_from_slice(&payload);
    Ok(buf)
}

/// Result of trying to decode one frame at the start of `buf`.
#[derive(Debug, PartialEq)]
pub enum DecodeOutcome {
    /// A full, valid frame starting at offset 0.
    Frame(WalFrame, usize),
    /// A partial frame: the WAL was torn by a crash. Stop recovery here.
    TornTail,
    /// A full frame whose checksum is wrong: corruption. Stop recovery here.
    Corrupt(String),
}

/// Decode one frame from the start of `buf`.
pub fn decode_frame(buf: &[u8]) -> DecodeOutcome {
    if buf.is_empty() {
        return DecodeOutcome::TornTail;
    }
    if buf.len() < WAL_FRAME_HEADER_LEN {
        return DecodeOutcome::TornTail;
    }
    if buf[0] != WAL_FRAME_MAGIC {
        return DecodeOutcome::TornTail;
    }
    let version = u16::from_le_bytes(buf[1..3].try_into().unwrap());
    if version != WAL_VERSION {
        return DecodeOutcome::Corrupt(format!("unsupported WAL version {version}"));
    }
    let seq = u64::from_le_bytes(buf[3..11].try_into().unwrap());
    let len = u32::from_le_bytes(buf[11..15].try_into().unwrap()) as usize;
    if buf.len() < WAL_FRAME_HEADER_LEN + len {
        return DecodeOutcome::TornTail;
    }
    let payload = &buf[WAL_FRAME_HEADER_LEN..WAL_FRAME_HEADER_LEN + len];
    let stored_crc = u32::from_le_bytes(buf[15..19].try_into().unwrap());
    let actual = crc32(payload);
    if stored_crc != actual {
        return DecodeOutcome::Corrupt(format!(
            "frame {seq} checksum mismatch (stored {stored_crc:#x}, computed {actual:#x})"
        ));
    }
    let op = match bincode::deserialize(payload) {
        Ok(op) => op,
        Err(e) => return DecodeOutcome::Corrupt(format!("frame {seq}: {e}")),
    };
    DecodeOutcome::Frame(WalFrame { seq, op }, WAL_FRAME_HEADER_LEN + len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::format::Record;
    use serde_json::json;

    fn sample_frame(seq: u64) -> WalFrame {
        WalFrame {
            seq,
            op: WalOp::Put(PersistedRecord::from(&Record {
                id: seq,
                vector: vec![0.1, 0.2, 0.3],
                metadata: json!({"tag": "test"}),
            })),
        }
    }

    #[test]
    fn frame_roundtrip() {
        let frame = sample_frame(7);
        let bytes = encode_frame(&frame).unwrap();
        assert_eq!(
            decode_frame(&bytes),
            DecodeOutcome::Frame(frame, bytes.len())
        );
    }

    #[test]
    fn empty_is_torn_tail() {
        assert_eq!(decode_frame(&[]), DecodeOutcome::TornTail);
    }

    #[test]
    fn truncated_frame_is_torn_tail() {
        let bytes = encode_frame(&sample_frame(1)).unwrap();
        assert_eq!(
            decode_frame(&bytes[..bytes.len() - 3]),
            DecodeOutcome::TornTail
        );
    }

    #[test]
    fn corrupted_payload_detected() {
        let mut bytes = encode_frame(&sample_frame(2)).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        assert!(matches!(decode_frame(&bytes), DecodeOutcome::Corrupt(_)));
    }

    #[test]
    fn multi_frame_scan() {
        let f0 = encode_frame(&sample_frame(0)).unwrap();
        let f1 = encode_frame(&sample_frame(1)).unwrap();
        let f2 = encode_frame(&sample_frame(2)).unwrap();
        let mut buf = f0;
        buf.extend_from_slice(&f1);
        buf.extend_from_slice(&f2);
        let mut off = 0;
        let mut seqs = vec![];
        while let DecodeOutcome::Frame(frame, consumed) = decode_frame(&buf[off..]) {
            seqs.push(frame.seq);
            off += consumed;
        }
        assert_eq!(seqs, vec![0, 1, 2]);
    }
}
