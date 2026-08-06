//! Binary on-disk formats for NovaDB snapshot and WAL files.
//!
//! Both formats are built from a few primitives designed by hand:
//!
//! * **Snapshot files** (`data/{collection}.nova`): a magic header followed by
//!   length-prefixed, checksummed records.
//! * **WAL files** (`wal/{collection}.log`): a sequence of fixed-prefix frames
//!   carrying serialized operations.
//!
//! Record payloads are serialized with bincode (see task.md tech stack).
//! Byte order is little-endian throughout.

use serde::{Deserialize, Serialize};

use crate::error::{NovaError, Result};

/// Magic bytes identifying a NovaDB snapshot file: `NOVA1\x00`.
pub const SNAPSHOT_MAGIC: &[u8; 8] = b"NOVA1\0\0\0";
/// Magic byte identifying a WAL frame.
pub const WAL_FRAME_MAGIC: u8 = 0xAD;
/// Snapshot file format version.
pub const SNAPSHOT_VERSION: u32 = 1;
/// WAL frame format version.
pub const WAL_VERSION: u16 = 1;

/// A stored record: an id, an embedding, and arbitrary JSON metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Record {
    pub id: u64,
    pub vector: Vec<f32>,
    pub metadata: serde_json::Value,
}

/// The bincode-serialized form of a record. Metadata travels as a JSON
/// string: bincode cannot encode `serde_json::Value` directly (it has no
/// `deserialize_any` support), so JSON is the interchange format for
/// metadata, exactly as it is at the API boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedRecord {
    pub id: u64,
    pub vector: Vec<f32>,
    pub metadata: String,
}

impl From<&Record> for PersistedRecord {
    fn from(record: &Record) -> Self {
        PersistedRecord {
            id: record.id,
            vector: record.vector.clone(),
            metadata: serde_json::to_string(&record.metadata)
                .expect("JSON metadata always serializes"),
        }
    }
}

impl TryFrom<PersistedRecord> for Record {
    type Error = NovaError;

    fn try_from(stored: PersistedRecord) -> std::result::Result<Self, Self::Error> {
        let metadata = serde_json::from_str(&stored.metadata)
            .map_err(|e| NovaError::Encoding(format!("invalid metadata JSON: {e}")))?;
        Ok(Record {
            id: stored.id,
            vector: stored.vector,
            metadata,
        })
    }
}

/// Convert a record to its persisted form.
pub fn to_persisted(record: &Record) -> PersistedRecord {
    PersistedRecord::from(record)
}

/// Snapshot file header (exactly 32 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotHeader {
    /// Dimensionality of all vectors in the collection.
    pub dim: u32,
    /// The next auto-assigned record id.
    pub next_id: u64,
    /// Number of live records in the snapshot.
    pub record_count: u32,
}

pub const SNAPSHOT_HEADER_LEN: usize = 8 + 4 + 4 + 8 + 8;

/// Serialize a record payload with bincode.
pub fn encode_record(record: &Record) -> Result<Vec<u8>> {
    let persisted = PersistedRecord::from(record);
    bincode::serialize(&persisted).map_err(NovaError::from)
}

/// Deserialize a record payload.
pub fn decode_record(bytes: &[u8]) -> Result<Record> {
    let persisted: PersistedRecord = bincode::deserialize(bytes).map_err(NovaError::from)?;
    Record::try_from(persisted)
}

/// Serialize the snapshot header to bytes (little-endian).
pub fn encode_snapshot_header(header: &SnapshotHeader) -> [u8; SNAPSHOT_HEADER_LEN] {
    let mut buf = [0u8; SNAPSHOT_HEADER_LEN];
    buf[..8].copy_from_slice(SNAPSHOT_MAGIC);
    buf[8..12].copy_from_slice(&SNAPSHOT_VERSION.to_le_bytes());
    buf[12..16].copy_from_slice(&header.dim.to_le_bytes());
    buf[16..24].copy_from_slice(&header.next_id.to_le_bytes());
    buf[24..28].copy_from_slice(&header.record_count.to_le_bytes());
    // bytes 28..32 reserved
    buf
}

/// Parse and validate a snapshot header. Returns an error if the magic or
/// version do not match.
pub fn decode_snapshot_header(buf: &[u8; SNAPSHOT_HEADER_LEN]) -> Result<SnapshotHeader> {
    if &buf[..8] != SNAPSHOT_MAGIC {
        return Err(NovaError::Corrupt("bad snapshot magic".into()));
    }
    let version = u32::from_le_bytes(buf[8..12].try_into().unwrap());
    if version != SNAPSHOT_VERSION {
        return Err(NovaError::Corrupt(format!(
            "unsupported snapshot version {version}"
        )));
    }
    Ok(SnapshotHeader {
        dim: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
        next_id: u64::from_le_bytes(buf[16..24].try_into().unwrap()),
        record_count: u32::from_le_bytes(buf[24..28].try_into().unwrap()),
    })
}

/// Encode one record into a snapshot block: `[len u32][crc32 u32][payload]`.
pub fn encode_snapshot_block(record: &Record) -> Result<Vec<u8>> {
    let payload = encode_record(record)?;
    let mut block = Vec::with_capacity(8 + payload.len());
    block.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    block.extend_from_slice(&crc32(&payload).to_le_bytes());
    block.extend_from_slice(&payload);
    Ok(block)
}

/// Decode one snapshot block, verifying its checksum.
pub fn decode_snapshot_block(buf: &[u8]) -> Result<(Record, usize)> {
    if buf.len() < 8 {
        return Err(NovaError::Corrupt("truncated snapshot block".into()));
    }
    let len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
    let stored_crc = u32::from_le_bytes(buf[4..8].try_into().unwrap());
    if buf.len() < 8 + len {
        return Err(NovaError::Corrupt("truncated snapshot block".into()));
    }
    let payload = &buf[8..8 + len];
    let actual = crc32(payload);
    if actual != stored_crc {
        return Err(NovaError::Corrupt(format!(
            "snapshot block checksum mismatch (stored {stored_crc:#x}, computed {actual:#x})"
        )));
    }
    let record = decode_record(payload)?;
    Ok((record, 8 + len))
}

/// Compute the IEEE CRC-32 checksum of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_known_value() {
        // The canonical CRC-32("123456789") = 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF43926);
    }

    #[test]
    fn snapshot_header_roundtrip() {
        let header = SnapshotHeader {
            dim: 128,
            next_id: 42,
            record_count: 7,
        };
        let bytes = encode_snapshot_header(&header);
        assert_eq!(bytes.len(), SNAPSHOT_HEADER_LEN);
        let decoded = decode_snapshot_header(&bytes).unwrap();
        assert_eq!(decoded, header);
    }

    #[test]
    fn snapshot_block_roundtrip() {
        let record = Record {
            id: 3,
            vector: vec![0.1, 0.5, -2.0],
            metadata: serde_json::json!({"category": "electronics", "price": 99.5}),
        };
        let block = encode_snapshot_block(&record).unwrap();
        let (decoded, consumed) = decode_snapshot_block(&block).unwrap();
        assert_eq!(decoded, record);
        assert_eq!(consumed, block.len());
    }

    #[test]
    fn snapshot_block_detects_corruption() {
        let record = Record {
            id: 1,
            vector: vec![1.0],
            metadata: serde_json::json!({}),
        };
        let mut block = encode_snapshot_block(&record).unwrap();
        block[8] ^= 0xFF; // flip a payload bit
        assert!(decode_snapshot_block(&block).is_err());
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut bytes = encode_snapshot_header(&SnapshotHeader {
            dim: 1,
            next_id: 0,
            record_count: 0,
        });
        bytes[0] = b'X';
        let err = decode_snapshot_header(&bytes).unwrap_err();
        assert!(matches!(err, NovaError::Corrupt(_)));
    }
}
