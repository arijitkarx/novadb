//! The `Database`: owns collections, WAL segments, and snapshot lifecycle.
//!
//! Durability contract (WAL-first):
//!
//! 1. A mutation is serialized into a WAL frame and appended to the log.
//! 2. Depending on [`SyncMode`], the log is fsynced.
//! 3. Only then is the mutation applied to the in-memory collection.
//!
//! On open, state is rebuilt as `snapshot + WAL replay`. Every WAL operation
//! is idempotent, so the sequence `snapshot` then `truncate WAL` is a safe
//! checkpoint even across a crash between the two steps.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::DbConfig;
use crate::error::{NovaError, Result};
use crate::metadata::filter::Filter;
use crate::storage::collection::Collection;
use crate::storage::format::Record;
use crate::vector::flat::SearchHit;
use crate::wal::frame::{WalFrame, WalOp};
use crate::wal::log::WalLog;
/// Per-collection state: in-memory data plus its WAL segment.
struct CollectionHandle {
    collection: Collection,
    wal: WalLog,
}

impl CollectionHandle {
    fn new(name: &str, dim: usize, wal: WalLog) -> Self {
        CollectionHandle {
            collection: Collection::new(name, dim),
            wal,
        }
    }
}

/// Statistics about one collection.
#[derive(Debug, Clone)]
pub struct CollectionStats {
    pub name: String,
    pub dim: usize,
    pub records: usize,
    pub next_id: u64,
    pub wal_frames: u64,
    pub ops_since_snapshot: u64,
}

/// An open NovaDB instance. Not `Sync`: the embedded model is one writer.
pub struct Database {
    config: DbConfig,
    collections: HashMap<String, CollectionHandle>,
}

impl Database {
    /// Open (or create) a database rooted at the configured directories.
    /// Loads all snapshots and replays all WAL segments.
    pub fn open(config: DbConfig) -> Result<Database> {
        std::fs::create_dir_all(&config.data_dir)?;
        std::fs::create_dir_all(&config.wal_dir)?;
        let mut db = Database {
            config,
            collections: HashMap::new(),
        };
        db.load_snapshots()?;
        db.load_wal_only_collections()?;
        Ok(db)
    }

    fn snapshot_path(&self, name: &str) -> PathBuf {
        self.config.data_dir.join(format!("{name}.nova"))
    }

    fn wal_path(&self, name: &str) -> PathBuf {
        self.config.wal_dir.join(format!("{name}.log"))
    }

    /// Load every snapshot file, then replay that collection's WAL.
    fn load_snapshots(&mut self) -> Result<()> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&self.config.data_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("nova") {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap()
                    .to_string();
                names.push(name);
            }
        }
        for name in names {
            let collection = Collection::load_snapshot(&self.snapshot_path(&name))?;
            let wal = WalLog::open(&self.config.wal_dir, &name, self.config.sync_mode)?;
            self.collections
                .insert(name.clone(), CollectionHandle { collection, wal });
            self.replay_wal(&name)?;
        }
        Ok(())
    }

    /// Load WAL segments that have no snapshot yet (collection created and
    /// written, but never checkpointed). The collection is materialized by
    /// replaying its `Create` frame.
    fn load_wal_only_collections(&mut self) -> Result<()> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&self.config.wal_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("log") {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap()
                    .to_string();
                if !self.collections.contains_key(&name) {
                    names.push(name);
                }
            }
        }
        for name in names {
            self.replay_wal(&name)?;
            if !self.collections.contains_key(&name) {
                // A log with real frames must always end up with a collection;
                // otherwise its Create frame is missing. Empty or torn-at-zero
                // logs are fine: nothing was ever durable.
                let empty = std::fs::metadata(self.wal_path(&name))?.len() == 0;
                if !empty {
                    return Err(NovaError::Corrupt(format!(
                        "WAL for '{name}' is missing a Create frame"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Replay a collection's WAL on top of its in-memory state, drop any torn
    /// tail, and sync the frame counter.
    fn replay_wal(&mut self, name: &str) -> Result<()> {
        let wal_path = self.wal_path(name);
        let (frames, durable_bytes) = WalLog::read_frames(&wal_path)?;
        let file_len = std::fs::metadata(&wal_path)?.len();

        // Open the log early so tail repair can happen before replay.
        let mut wal = WalLog::open(&self.config.wal_dir, name, self.config.sync_mode)?;

        if frames.is_empty() && file_len > 0 {
            // Nothing parsed. If the file starts with the frame magic it is a
            // torn first frame (crash mid-write): nothing durable, safe to
            // reset. Otherwise the log is unrecognizable: fail loudly rather
            // than silently discard possibly-real data.
            let first = {
                use std::io::Read;
                let mut f = std::fs::File::open(&wal_path)?;
                let mut b = [0u8; 1];
                let read = f.read(&mut b)?;
                if read == 0 {
                    0u8
                } else {
                    b[0]
                }
            };
            if first == crate::storage::format::WAL_FRAME_MAGIC {
                wal.truncate_to(0)?;
            } else {
                return Err(NovaError::Corrupt(format!(
                    "{}: unrecognized WAL content",
                    wal_path.display()
                )));
            }
        } else if durable_bytes < file_len {
            // Crash left a torn tail: forget it, but keep the durable prefix.
            wal.truncate_to(durable_bytes)?;
        }

        let frame_count = frames.len() as u64;
        for frame in frames {
            self.apply_frame(name, frame)?;
        }

        // Wire the replayed log into the handle. A WAL-only collection is
        // materialized by its Create frame during apply_frame; if nothing was
        // replayed (empty log, or a torn first frame), the collection may
        // legitimately not exist — its creation was never durable.
        let Some(handle) = self.collections.get_mut(name) else {
            return Ok(());
        };
        handle.wal = wal;
        handle.wal.set_frame_count(frame_count);
        handle.collection.reset_op_count();
        Ok(())
    }

    fn apply_frame(&mut self, name: &str, frame: WalFrame) -> Result<()> {
        match frame.op {
            WalOp::Create(dim) => {
                if !self.collections.contains_key(name) {
                    let wal = WalLog::open(&self.config.wal_dir, name, self.config.sync_mode)?;
                    self.collections.insert(
                        name.to_string(),
                        CollectionHandle::new(name, dim as usize, wal),
                    );
                }
            }
            WalOp::Put(persisted) => {
                let handle = self.collections.get_mut(name).ok_or_else(|| {
                    NovaError::Corrupt(format!("WAL for '{name}': Put before Create"))
                })?;
                let record = Record::try_from(persisted)?;
                handle
                    .collection
                    .apply_put(record)
                    .map_err(|e| NovaError::Corrupt(format!("WAL replay of '{name}': {e}")))?;
            }
            WalOp::Delete(id) => {
                if let Some(handle) = self.collections.get_mut(name) {
                    handle.collection.apply_delete(id);
                }
            }
        }
        Ok(())
    }

    /// List all collection names, sorted.
    pub fn list_collections(&self) -> Vec<String> {
        let mut names: Vec<String> = self.collections.keys().cloned().collect();
        names.sort();
        names
    }

    /// Statistics for one collection.
    pub fn stats(&self, name: &str) -> Result<CollectionStats> {
        let handle = self.handle(name)?;
        Ok(CollectionStats {
            name: handle.collection.name.clone(),
            dim: handle.collection.dim,
            records: handle.collection.len(),
            next_id: handle.collection.next_id(),
            wal_frames: handle.wal.frame_count(),
            ops_since_snapshot: handle.collection.op_count(),
        })
    }

    /// Create a collection with the given vector dimensionality.
    pub fn create_collection(&mut self, name: &str, dim: usize) -> Result<()> {
        validate_name(name)?;
        if self.collections.contains_key(name) {
            return Err(NovaError::CollectionExists(name.into()));
        }
        if dim == 0 {
            return Err(NovaError::EmptyVector);
        }
        let wal = WalLog::open(&self.config.wal_dir, name, self.config.sync_mode)?;
        let mut handle = CollectionHandle::new(name, dim, wal);
        handle.wal.append_create(dim as u32)?;
        handle.collection.reset_op_count();
        self.collections.insert(name.to_string(), handle);
        Ok(())
    }

    /// Drop a collection and delete its snapshot and WAL files.
    pub fn drop_collection(&mut self, name: &str) -> Result<()> {
        if self.collections.remove(name).is_none() {
            return Err(NovaError::CollectionNotFound(name.into()));
        }
        for path in [self.snapshot_path(name), self.wal_path(name)] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Insert a record with an explicit id (upsert semantics).
    pub fn insert(
        &mut self,
        collection: &str,
        id: u64,
        vector: &[f32],
        metadata: serde_json::Value,
    ) -> Result<()> {
        let handle = self.handle_mut(collection)?;
        handle.collection.check_vector(vector)?;
        let record = Record {
            id,
            vector: vector.to_vec(),
            metadata,
        };
        handle.wal.append_put(&record)?;
        handle.collection.apply_put(record)?;
        self.maybe_auto_snapshot(collection)?;
        Ok(())
    }

    /// Insert a record with an auto-assigned id; returns the id.
    pub fn add(
        &mut self,
        collection: &str,
        vector: &[f32],
        metadata: serde_json::Value,
    ) -> Result<u64> {
        let handle = self.handle_mut(collection)?;
        handle.collection.check_vector(vector)?;
        let id = handle.collection.alloc_id();
        let record = Record {
            id,
            vector: vector.to_vec(),
            metadata,
        };
        handle.wal.append_put(&record)?;
        handle.collection.apply_put(record)?;
        self.maybe_auto_snapshot(collection)?;
        Ok(id)
    }

    /// Delete a record by id. Returns `true` if it existed.
    pub fn delete(&mut self, collection: &str, id: u64) -> Result<bool> {
        let handle = self.handle_mut(collection)?;
        if handle.collection.get(id).is_none() {
            return Ok(false);
        }
        handle.wal.append_delete(id)?;
        handle.collection.apply_delete(id);
        self.maybe_auto_snapshot(collection)?;
        Ok(true)
    }

    /// Fetch a record by id.
    pub fn get(&self, collection: &str, id: u64) -> Result<Option<Record>> {
        Ok(self.handle(collection)?.collection.get(id).cloned())
    }

    /// All records of a collection, in insertion order.
    pub fn all_records(&self, collection: &str) -> Result<Vec<Record>> {
        Ok(self
            .handle(collection)?
            .collection
            .iter()
            .cloned()
            .collect())
    }

    /// Run a hybrid query; see [`crate::engine::query::Query`].
    pub fn query(&self, query: &crate::engine::query::Query) -> Result<Vec<SearchHit>> {
        query.run(self)
    }

    /// Snapshot one collection and truncate its WAL.
    pub fn snapshot(&mut self, collection: &str) -> Result<()> {
        let path = self.snapshot_path(collection);
        let handle = self.handle_mut(collection)?;
        handle.collection.write_snapshot_atomic(&path)?;
        handle.wal.truncate()?;
        handle.collection.reset_op_count();
        Ok(())
    }

    /// Snapshot every collection (a full checkpoint).
    pub fn snapshot_all(&mut self) -> Result<()> {
        let names = self.list_collections();
        for name in names {
            self.snapshot(&name)?;
        }
        Ok(())
    }

    /// Flush everything to disk and close cleanly.
    pub fn close(&mut self) -> Result<()> {
        self.snapshot_all()
    }

    /// Write a snapshot when the WAL has grown past the configured threshold.
    fn maybe_auto_snapshot(&mut self, name: &str) -> Result<()> {
        let should = {
            let handle = self.handle(name)?;
            handle.collection.op_count() >= self.config.auto_snapshot_after
        };
        if should {
            self.snapshot(name)?;
        }
        Ok(())
    }

    fn handle(&self, name: &str) -> Result<&CollectionHandle> {
        self.collections
            .get(name)
            .ok_or_else(|| NovaError::CollectionNotFound(name.into()))
    }

    fn handle_mut(&mut self, name: &str) -> Result<&mut CollectionHandle> {
        self.collections
            .get_mut(name)
            .ok_or_else(|| NovaError::CollectionNotFound(name.into()))
    }

    /// Public accessor for the query engine.
    pub(crate) fn collection(&self, name: &str) -> Result<&Collection> {
        Ok(&self.handle(name)?.collection)
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        // Best-effort checkpoint on drop (e.g. on panic unwinding).
        let _ = self.snapshot_all();
    }
}

/// Collection names are identifiers: `[a-zA-Z0-9_-]{1,64}`, never "." or "..".
fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(NovaError::Parse(format!(
            "invalid collection name '{name}'"
        )))
    }
}

pub(crate) fn filter_predicate<'a>(filter: Option<&'a Filter>) -> impl Fn(&Record) -> bool + 'a {
    move |record: &Record| match filter {
        None => true,
        Some(f) => f.matches(&record.metadata),
    }
}
