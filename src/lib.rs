//! NovaDB — a lightweight embedded vector database built from scratch in Rust.
//!
//! NovaDB implements its own storage engine, write-ahead log with crash
//! recovery and snapshots, exact flat vector search with cosine similarity,
//! metadata filtering, and hybrid (filter + vector) queries.
//!
//! # Quick start
//!
//! ```no_run
//! use novadb::engine::query::Query;
//! use novadb::metadata::Filter;
//! use novadb::{Database, DbConfig};
//!
//! fn main() -> novadb::Result<()> {
//!     let mut db = Database::open(DbConfig::new("/tmp/novadb"))?;
//!
//!     db.create_collection("products", 3)?;
//!     db.add(
//!         "products",
//!         &[0.1, 0.2, 0.3],
//!         serde_json::json!({"category": "electronics", "price": 99}),
//!     )?;
//!
//!     let hits = Query::new("products", &[0.1, 0.2, 0.3])
//!         .top_k(5)
//!         .filter(Filter::parse("price < 1000")?)
//!         .run(&db)?;
//!     println!("{} hits", hits.len());
//!
//!     db.close()?;
//!     Ok(())
//! }
//! ```
//!
//! See `docs/` for the on-disk format, the WAL protocol, and the query DSL.

pub mod config;
pub mod engine;
pub mod error;
pub mod metadata;
pub mod storage;
pub mod vector;
pub mod wal;

pub use config::{DbConfig, SyncMode};
pub use engine::Database;
pub use error::{NovaError, Result};
pub use storage::Record;
