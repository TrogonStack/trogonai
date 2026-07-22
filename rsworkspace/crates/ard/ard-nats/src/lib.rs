#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! NATS storage and indexing primitives for ARD-compatible catalog data.

pub mod catalog_event;
pub mod catalog_index;
pub mod catalog_subject;
pub mod memory_catalog_store;
pub mod store;

pub use catalog_event::{CatalogEvent, CatalogEventWire};
pub use catalog_index::CatalogIndex;
pub use catalog_subject::{CatalogEventSubject, CatalogSubjectKind};
pub use memory_catalog_store::MemoryCatalogStore;
pub use store::{CatalogStore, CatalogStoreError};
