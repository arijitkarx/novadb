//! The storage engine: on-disk formats and in-memory collection state.

pub mod collection;
pub mod format;

pub use collection::Collection;
pub use format::Record;
