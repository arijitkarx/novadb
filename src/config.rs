use std::path::PathBuf;

/// How aggressively the WAL is flushed to disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncMode {
    /// fsync the WAL after every single write. Maximum durability, slowest.
    #[default]
    Every,
    /// fsync the WAL every `n` writes. Crash may lose at most the last `n` writes.
    Batch(u64),
    /// Never fsync. Fastest; relies on the OS to eventually flush.
    Never,
}

/// Configuration for opening a [`crate::engine::Database`].
#[derive(Debug, Clone)]
pub struct DbConfig {
    /// Directory where collection snapshot (`.nova`) files live.
    pub data_dir: PathBuf,
    /// Directory where WAL (`.log`) files live.
    pub wal_dir: PathBuf,
    /// WAL flush policy.
    pub sync_mode: SyncMode,
    /// Number of appended WAL ops after which a collection is automatically
    /// snapshot and its WAL truncated.
    pub auto_snapshot_after: u64,
}

impl DbConfig {
    /// Create a config rooted at `dir`, with `dir` holding data and `dir/wal`
    /// holding the WAL.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let wal_dir = dir.join("wal");
        DbConfig {
            data_dir: dir,
            wal_dir,
            sync_mode: SyncMode::default(),
            auto_snapshot_after: 10_000,
        }
    }

    /// Set the WAL sync policy.
    pub fn with_sync_mode(mut self, mode: SyncMode) -> Self {
        self.sync_mode = mode;
        self
    }

    /// Set the auto-snapshot threshold (ops appended before WAL compaction).
    pub fn with_auto_snapshot_after(mut self, n: u64) -> Self {
        self.auto_snapshot_after = n;
        self
    }
}
