//! In-memory collection state and snapshot (de)serialization.
//!
//! A [`Collection`] is the runtime representation of a vector collection: an
//! append-ordered record list plus an id -> index map for O(1) point lookups.
//! All mutation happens by *applying operations*; the storage engine is
//! responsible for persisting those ops to the WAL *before* applying them to
//! memory, so the two stay consistent across crashes.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use crate::error::{NovaError, Result};
use crate::storage::format::{
    decode_record, decode_snapshot_block, decode_snapshot_header, encode_record,
    encode_snapshot_block, encode_snapshot_header, Record, SnapshotHeader, SNAPSHOT_HEADER_LEN,
};

/// A named collection of vectors with metadata.
pub struct Collection {
    pub name: String,
    /// Dimensionality shared by every vector in the collection.
    pub dim: usize,
    /// Live records in insertion order (swapped on delete).
    items: Vec<Record>,
    /// id -> position in `items` (kept in sync across swap-removes).
    id_to_index: HashMap<u64, usize>,
    /// Precomputed L2 norms of `items`, so cosine similarity skips sqrt.
    norms: Vec<f32>,
    /// Next auto-assigned id (persisted in snapshots).
    next_id: u64,
    /// Number of ops applied since the last snapshot (drives auto-snapshot).
    op_count: u64,
}

impl Collection {
    /// Create a new, empty in-memory collection.
    pub fn new(name: impl Into<String>, dim: usize) -> Self {
        Collection {
            name: name.into(),
            dim,
            items: Vec::new(),
            id_to_index: HashMap::new(),
            norms: Vec::new(),
            next_id: 1,
            op_count: 0,
        }
    }

    /// Number of live records.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Ops applied since the last snapshot.
    pub fn op_count(&self) -> u64 {
        self.op_count
    }

    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    /// Fetch a record by id.
    pub fn get(&self, id: u64) -> Option<&Record> {
        let idx = *self.id_to_index.get(&id)?;
        Some(&self.items[idx])
    }

    /// Insert or replace a record. Rejects mismatched dimensionality and
    /// duplicate ids in a way the caller must resolve (see `put` below
    /// generates fresh ids; explicit-id puts error on duplicates).
    pub fn apply_put(&mut self, record: Record) -> Result<()> {
        self.check_vector(&record.vector)?;
        let id = record.id;
        if let Some(&idx) = self.id_to_index.get(&record.id) {
            // Replace in place: same id, same slot.
            self.items[idx] = record.clone();
            self.norms[idx] = l2_norm(&record.vector);
        } else {
            self.id_to_index.insert(record.id, self.items.len());
            self.norms.push(l2_norm(&record.vector));
            self.items.push(record);
        }
        if self.next_id <= id {
            self.next_id = id + 1;
        }
        self.op_count += 1;
        Ok(())
    }

    /// Remove a record by id. Returns `true` if the record existed.
    pub fn apply_delete(&mut self, id: u64) -> bool {
        let Some(idx) = self.id_to_index.remove(&id) else {
            return false;
        };
        let last = self.items.len() - 1;
        if idx != last {
            // Swap the tail record into the hole and fix its index.
            let swapped_id = self.items[last].id;
            self.items[idx] = self.items[last].clone();
            self.norms[idx] = self.norms[last];
            self.id_to_index.insert(swapped_id, idx);
        }
        self.items.pop();
        self.norms.pop();
        self.op_count += 1;
        true
    }

    /// Allocate a fresh auto-generated id.
    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Iterate over all live records (in insertion order).
    pub fn iter(&self) -> impl Iterator<Item = &Record> {
        self.items.iter()
    }

    /// Precomputed norm of the record at `index`.
    pub fn norm(&self, index: usize) -> f32 {
        self.norms[index]
    }

    /// Number of live records.
    pub fn record_count(&self) -> usize {
        self.items.len()
    }

    /// Validate a vector's dimensionality against this collection.
    pub fn check_vector(&self, vector: &[f32]) -> Result<()> {
        if vector.is_empty() {
            return Err(NovaError::EmptyVector);
        }
        if vector.len() != self.dim {
            return Err(NovaError::DimMismatch {
                expected: self.dim,
                got: vector.len(),
            });
        }
        Ok(())
    }

    /// Reset the op counter (used after WAL replay, so an opened database
    /// doesn't immediately auto-snapshot).
    pub fn reset_op_count(&mut self) {
        self.op_count = 0;
    }

    /// Load a collection from a snapshot file.
    pub fn load_snapshot(path: &Path) -> Result<Collection> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut header_buf = [0u8; SNAPSHOT_HEADER_LEN];
        reader.read_exact(&mut header_buf)?;
        let header = decode_snapshot_header(&header_buf)?;

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| NovaError::Corrupt("snapshot path has no valid file name".into()))?
            .to_string();

        let mut collection = Collection {
            name,
            dim: header.dim as usize,
            items: Vec::with_capacity(header.record_count as usize),
            id_to_index: HashMap::with_capacity(header.record_count as usize),
            norms: Vec::with_capacity(header.record_count as usize),
            next_id: header.next_id,
            op_count: 0,
        };

        let mut rest = Vec::new();
        reader.read_to_end(&mut rest)?;
        let mut offset = 0usize;
        let mut seen = 0u32;
        while offset < rest.len() {
            let (record, consumed) = decode_snapshot_block(&rest[offset..])?;
            collection
                .id_to_index
                .insert(record.id, collection.items.len());
            collection.norms.push(l2_norm(&record.vector));
            collection.items.push(record);
            offset += consumed;
            seen += 1;
        }
        if seen != header.record_count {
            return Err(NovaError::Corrupt(format!(
                "snapshot record count mismatch: header says {}, file has {}",
                header.record_count, seen
            )));
        }
        Ok(collection)
    }

    /// Write the collection to a snapshot file. The caller is responsible for
    /// atomicity (write to a temp file, then rename).
    pub fn write_snapshot_to(&self, file: &mut File) -> Result<()> {
        let header = SnapshotHeader {
            dim: self.dim as u32,
            next_id: self.next_id,
            record_count: self.items.len() as u32,
        };
        let mut writer = BufWriter::new(file);
        writer.write_all(&encode_snapshot_header(&header))?;
        for record in &self.items {
            let block = encode_snapshot_block(record)?;
            writer.write_all(&block)?;
        }
        writer.flush()?;
        Ok(())
    }

    /// Atomically persist this collection to `path`: write `path.tmp` then
    /// rename over `path`.
    pub fn write_snapshot_atomic(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("nova.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)?;
        self.write_snapshot_to(&mut file)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Load a collection from raw snapshot bytes (used in tests).
    #[cfg(test)]
    pub fn load_snapshot_bytes(bytes: &[u8], name: &str) -> Result<Collection> {
        let mut header_buf = [0u8; SNAPSHOT_HEADER_LEN];
        header_buf.copy_from_slice(&bytes[..SNAPSHOT_HEADER_LEN]);
        let header = decode_snapshot_header(&header_buf)?;
        let mut collection = Collection {
            name: name.to_string(),
            dim: header.dim as usize,
            items: Vec::new(),
            id_to_index: HashMap::new(),
            norms: Vec::new(),
            next_id: header.next_id,
            op_count: 0,
        };
        let mut offset = SNAPSHOT_HEADER_LEN;
        while offset < bytes.len() {
            let (record, consumed) = decode_snapshot_block(&bytes[offset..])?;
            collection
                .id_to_index
                .insert(record.id, collection.items.len());
            collection.norms.push(l2_norm(&record.vector));
            collection.items.push(record);
            offset += consumed;
        }
        Ok(collection)
    }

    /// Serialize the collection to raw snapshot bytes (used in tests).
    #[cfg(test)]
    pub fn to_snapshot_bytes(&self) -> Result<Vec<u8>> {
        let header = SnapshotHeader {
            dim: self.dim as u32,
            next_id: self.next_id,
            record_count: self.items.len() as u32,
        };
        let mut out = encode_snapshot_header(&header).to_vec();
        for record in &self.items {
            out.extend_from_slice(&encode_snapshot_block(record)?);
        }
        Ok(out)
    }
}

/// L2 norm of a vector; 0.0 for the zero vector.
pub fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

/// Encode a record payload (re-exported for the WAL module).
pub fn serialize_record(record: &Record) -> Result<Vec<u8>> {
    encode_record(record)
}

/// Decode a record payload.
pub fn deserialize_record(bytes: &[u8]) -> Result<Record> {
    decode_record(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(id: u64, x: f32) -> Record {
        Record {
            id,
            vector: vec![x, x + 1.0],
            metadata: json!({"i": id}),
        }
    }

    #[test]
    fn put_get_delete() {
        let mut c = Collection::new("t", 2);
        c.apply_put(rec(1, 1.0)).unwrap();
        c.apply_put(rec(2, 2.0)).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c.get(1).unwrap().vector, vec![1.0, 2.0]);
        assert!(c.apply_delete(1));
        assert!(!c.apply_delete(99));
        assert_eq!(c.len(), 1);
        assert_eq!(c.get(2).unwrap().id, 2);
    }

    #[test]
    fn delete_swaps_keep_map_consistent() {
        let mut c = Collection::new("t", 2);
        for i in 1..=5 {
            c.apply_put(rec(i, i as f32)).unwrap();
        }
        c.apply_delete(2);
        c.apply_put(rec(6, 6.0)).unwrap();
        c.apply_delete(5);
        for id in [1, 3, 4, 6] {
            assert_eq!(c.get(id).unwrap().id, id);
        }
        assert_eq!(c.len(), 4);
    }

    #[test]
    fn replace_in_place_keeps_order() {
        let mut c = Collection::new("t", 2);
        c.apply_put(rec(1, 1.0)).unwrap();
        c.apply_put(rec(2, 2.0)).unwrap();
        c.apply_put(Record {
            id: 1,
            vector: vec![9.0, 9.0],
            metadata: json!({}),
        })
        .unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c.get(1).unwrap().vector, vec![9.0, 9.0]);
    }

    #[test]
    fn dim_mismatch_rejected() {
        let mut c = Collection::new("t", 2);
        assert!(matches!(
            c.apply_put(Record {
                id: 1,
                vector: vec![1.0],
                metadata: json!({})
            }),
            Err(NovaError::DimMismatch {
                expected: 2,
                got: 1
            })
        ));
        assert!(matches!(
            c.apply_put(Record {
                id: 1,
                vector: vec![],
                metadata: json!({})
            }),
            Err(NovaError::EmptyVector)
        ));
    }

    #[test]
    fn auto_ids_are_monotonic() {
        let mut c = Collection::new("t", 2);
        assert_eq!(c.alloc_id(), 1);
        assert_eq!(c.alloc_id(), 2);
        // Explicit ids advance the counter past them.
        c.apply_put(Record {
            id: 10,
            vector: vec![0.0, 0.0],
            metadata: json!({}),
        })
        .unwrap();
        assert_eq!(c.alloc_id(), 11);
    }

    #[test]
    fn snapshot_roundtrip() {
        let mut c = Collection::new("t", 2);
        for i in 1..=10 {
            c.apply_put(rec(i, i as f32)).unwrap();
        }
        c.apply_delete(3);
        let bytes = c.to_snapshot_bytes().unwrap();
        let loaded = Collection::load_snapshot_bytes(&bytes, "t").unwrap();
        assert_eq!(loaded.len(), 9);
        assert_eq!(loaded.next_id, 11);
        assert_eq!(loaded.get(1).unwrap().vector, vec![1.0, 2.0]);
        assert!(loaded.get(3).is_none());
    }
}
