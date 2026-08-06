//! The write-ahead log: a single append-only file of framed operations.
//!
//! The WAL is the *source of truth* for durability. A mutation is only
//! applied to the in-memory collection after it has been appended to the WAL
//! (and optionally fsynced). After a crash, the database rebuilds state by
//! loading the last snapshot and replaying the WAL from the beginning.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};

use crate::config::SyncMode;
use crate::error::{NovaError, Result};
use crate::storage::format::{to_persisted, Record};
use crate::wal::frame::{decode_frame, encode_frame, DecodeOutcome, WalFrame, WalOp};

/// A WAL segment backing one collection.
pub struct WalLog {
    path: PathBuf,
    file: File,
    /// Number of frames appended since this log was last truncated.
    frame_count: u64,
    /// Frames appended since the last fsync.
    unsynced: u64,
    sync_mode: SyncMode,
}

impl WalLog {
    /// Open (creating if needed) the WAL for `collection` in `wal_dir`.
    pub fn open(wal_dir: &Path, collection: &str, sync_mode: SyncMode) -> Result<WalLog> {
        std::fs::create_dir_all(wal_dir)?;
        let path = wal_dir.join(format!("{collection}.log"));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(WalLog {
            path,
            file,
            frame_count: 0,
            unsynced: 0,
            sync_mode,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    /// Append a `Create` operation to the log.
    pub fn append_create(&mut self, dim: u32) -> Result<u64> {
        self.append(&WalFrame {
            seq: self.frame_count,
            op: WalOp::Create(dim),
        })
    }

    /// Append a `Put` operation to the log.
    pub fn append_put(&mut self, record: &Record) -> Result<u64> {
        self.append(&WalFrame {
            seq: self.frame_count,
            op: WalOp::Put(to_persisted(record)),
        })
    }

    /// Append a `Delete` operation to the log.
    pub fn append_delete(&mut self, id: u64) -> Result<u64> {
        self.append(&WalFrame {
            seq: self.frame_count,
            op: WalOp::Delete(id),
        })
    }

    /// Append a frame and honor the sync policy.
    fn append(&mut self, frame: &WalFrame) -> Result<u64> {
        let bytes = encode_frame(frame)?;
        self.file.write_all(&bytes)?;
        self.frame_count += 1;
        self.unsynced += 1;
        match self.sync_mode {
            SyncMode::Every => self.file.sync_all()?,
            SyncMode::Batch(n) => {
                if n > 0 && self.unsynced >= n {
                    self.file.sync_all()?;
                    self.unsynced = 0;
                }
            }
            SyncMode::Never => {}
        }
        Ok(frame.seq)
    }

    /// Truncate the log back to zero frames. Used after a successful
    /// checkpoint, when the snapshot already reflects all applied ops.
    pub fn truncate(&mut self) -> Result<()> {
        self.truncate_to(0)
    }

    /// Truncate the log to `bytes` (used to drop a torn tail after recovery).
    pub fn truncate_to(&mut self, bytes: u64) -> Result<()> {
        self.file.set_len(bytes)?;
        self.file.seek(std::io::SeekFrom::Start(bytes))?;
        self.file.sync_all()?;
        self.frame_count = 0;
        self.unsynced = 0;
        Ok(())
    }

    /// Overwrite the frame counter (used after replaying a recovered log).
    pub fn set_frame_count(&mut self, count: u64) {
        self.frame_count = count;
    }

    /// Read every valid frame in a log file. Stops cleanly at the first torn
    /// tail or corrupted frame; reports the number of bytes that were
    /// successfully parsed (this is the durable prefix).
    pub fn read_frames(path: &Path) -> Result<(Vec<WalFrame>, u64)> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        let mut frames = Vec::new();
        let mut offset = 0usize;
        loop {
            match decode_frame(&buf[offset..]) {
                DecodeOutcome::Frame(frame, consumed) => {
                    frames.push(frame);
                    offset += consumed;
                }
                DecodeOutcome::TornTail => break,
                DecodeOutcome::Corrupt(msg) => {
                    return Err(NovaError::Corrupt(format!(
                        "{} ({}): {msg}",
                        path.display(),
                        offset
                    )))
                }
            }
        }
        Ok((frames, offset as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(id: u64) -> Record {
        Record {
            id,
            vector: vec![id as f32],
            metadata: json!({}),
        }
    }

    #[test]
    fn append_and_replay() {
        let dir = tempfile::tempdir().unwrap();
        let sync_mode = SyncMode::Every;
        {
            let mut wal = WalLog::open(dir.path(), "c", sync_mode).unwrap();
            wal.append_put(&record(1)).unwrap();
            wal.append_put(&record(2)).unwrap();
            wal.append_delete(1).unwrap();
        }
        let (frames, durable_bytes) = WalLog::read_frames(&dir.path().join("c.log")).unwrap();
        assert_eq!(
            durable_bytes as usize,
            std::fs::metadata(dir.path().join("c.log")).unwrap().len() as usize
        );
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].seq, 0);
        assert_eq!(frames[2].op, WalOp::Delete(1));
    }

    #[test]
    fn torn_tail_is_recovered_from() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut wal = WalLog::open(dir.path(), "c", SyncMode::Every).unwrap();
            wal.append_put(&record(1)).unwrap();
            wal.append_put(&record(2)).unwrap();
            wal.append_put(&record(3)).unwrap();
        }
        let path = dir.path().join("c.log");
        // Simulate a crash mid-write: truncate the last frame.
        let len = std::fs::metadata(&path).unwrap().len();
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(len - 5).unwrap();
        let (frames, _) = WalLog::read_frames(&path).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].op, WalOp::Put(to_persisted(&record(2))));
    }

    #[test]
    fn truncate_resets_frame_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut wal = WalLog::open(dir.path(), "c", SyncMode::Every).unwrap();
        wal.append_put(&record(1)).unwrap();
        wal.append_put(&record(2)).unwrap();
        wal.truncate().unwrap();
        assert_eq!(wal.frame_count(), 0);
        assert_eq!(std::fs::metadata(wal.path()).unwrap().len(), 0);
        let (frames, _) = WalLog::read_frames(wal.path()).unwrap();
        assert!(frames.is_empty());
    }
}
