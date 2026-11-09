//! Provider-neutral envelope-wrapping values. Provider authentication and service admission belong to their owners.

mod key_ref;
pub use key_ref::{KeyRef, KeyRefError};
mod tenant_id;
pub use tenant_id::{TenantId, TenantIdError};
mod resource_type_id;
pub use resource_type_id::{ResourceTypeId, ResourceTypeIdError};
mod resource_id;
pub use resource_id::{ResourceId, ResourceIdError};
mod key_purpose;
pub use key_purpose::{KeyPurpose, KeyPurposeError};
mod binding_version;
pub use binding_version::{BindingVersion, BindingVersionError};
mod material_suite;
pub use material_suite::{MaterialSuite, MaterialSuiteError};
mod authenticated_binding;
pub use authenticated_binding::AuthenticatedBinding;
mod wrapped_dek;
pub use wrapped_dek::{WrappedDek, WrappedDekError};
mod dek;
pub use dek::{Dek, DekError};

mod key_use_denial_reason;
pub use key_use_denial_reason::KeyUseDenialReason;
mod key_operation_error;
pub use key_operation_error::KeyOperationError;
mod wrap_dek;
pub use wrap_dek::WrapDek;
mod unwrap_dek;
pub use unwrap_dek::UnwrapDek;

#[cfg(feature = "test-support")]
pub mod test_support;
