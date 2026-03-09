//! In-memory L2 order book engine with sorted BTreeMap sides. See [README](../README.md).

pub mod book;
pub mod error;

pub use book::L2Book;
pub use error::BookError;
