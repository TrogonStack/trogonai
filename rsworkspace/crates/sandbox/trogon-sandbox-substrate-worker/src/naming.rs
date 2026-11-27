//! The join key between our lease ids and Substrate's resource names.
//!
//! An `Actor` carries no labels or annotations (its `ResourceMetadata` is atespace, name,
//! uid, version and timestamps), so there is nowhere to stash a lease id on the Substrate
//! side. The name has to carry the association by construction.

mod actor_name;
mod atespace;
mod constants;
mod dns_label;
mod dns_label_error;
mod managed;
mod naming_error;

pub use actor_name::ActorName;
pub use atespace::Atespace;
pub use managed::is_managed;
pub use naming_error::NamingError;

#[cfg(test)]
mod tests;
