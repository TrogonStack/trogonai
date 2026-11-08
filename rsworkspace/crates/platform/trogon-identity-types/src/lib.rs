//! Shared mesh identity types: principals and the form a subject carries
//! them in, the nested `act` delegation chain, AAuth claims.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod aauth;
pub mod constants;
mod principal_id;
mod principal_kind;
mod subject_principal;
mod subject_principal_id;

pub use principal_id::{PrincipalId, PrincipalIdError};
pub use principal_kind::{PrincipalKind, PrincipalKindError};
pub use subject_principal::{SubjectPrincipal, SubjectPrincipalError};
pub use subject_principal_id::{SubjectPrincipalId, SubjectPrincipalIdError};
