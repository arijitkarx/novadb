//! The write-ahead log: framing, durable append, and recovery replay.

pub mod frame;
pub mod log;

pub use frame::WalOp;
pub use log::WalLog;
