//! Renders each sandbox worker's declared [`ProviderCapabilityProfile`] into the markdown tables in this
//! crate's `README.md`.
//!
//! Every profile is serialized with serde and flattened into dotted paths, so a field added to the profile
//! structs shows up as a new row without touching this crate. A new worker is one entry in [`columns`].
//!
//! [`ProviderCapabilityProfile`]: trogon_sandbox_contract::ProviderCapabilityProfile

mod column;
mod columns;
mod constants;
mod doc;
mod marked_doc;
mod matrix_error;
mod mode;
mod render;
mod unopened_volume_stores;

pub use column::Column;
pub use columns::columns;
pub use constants::{BEGIN_MARKER, END_MARKER, PLATFORM_DEPENDENT, REGENERATE_COMMAND};
pub use doc::{check_doc, doc_path, expected_doc, write_doc};
pub use marked_doc::MarkedDoc;
pub use matrix_error::MatrixError;
pub use mode::Mode;
pub use render::render;
