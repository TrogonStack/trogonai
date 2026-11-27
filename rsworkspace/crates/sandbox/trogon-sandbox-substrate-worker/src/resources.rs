//! What an actor is given to run with, and which agent images may back one.

mod actor_resources;
mod actor_resources_error;
mod constants;
mod image_digest_error;
mod image_reference;

pub use actor_resources::ActorResources;
pub use actor_resources_error::ActorResourcesError;
pub use image_digest_error::ImageDigestError;
pub use image_reference::validate_image_digest;

#[cfg(test)]
mod tests;
