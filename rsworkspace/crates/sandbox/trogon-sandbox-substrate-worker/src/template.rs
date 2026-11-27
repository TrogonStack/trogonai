//! Content-addressed `ActorTemplate` registry.
//!
//! `CreateActor` takes a template ref, not an image, and a template only becomes usable
//! after Substrate's `ActorTemplateReconciler` has built a golden snapshot for it. A lease
//! therefore cannot mint a template on the dispatch path; it can only converge on one that
//! already exists or wait for the build it triggered.
//!
//! Two properties make that safe under concurrency:
//!
//! - The template name is a hash of everything that would invalidate the golden snapshot, so
//!   two workers computing a name for the same workload compute the *same* name and
//!   `CreateActorTemplate` collapses to `ALREADY_EXISTS`.
//! - A NATS KV *create* (not put) elects exactly one worker to drive each build, so a burst
//!   of leases on a cold image produces one build and N waiters instead of N builds.

mod build_template;
mod constants;
mod nats_template_index;
mod now_secs;
mod template_error;
mod template_index;
mod template_index_error;
mod template_key;
mod template_phase;
mod template_record;
mod template_registry;

pub use constants::TEMPLATE_KV_BUCKET;
pub use nats_template_index::NatsTemplateIndex;
pub use template_error::TemplateError;
pub use template_index::TemplateIndex;
pub use template_index_error::TemplateIndexError;
pub use template_key::TemplateKey;
pub use template_phase::TemplatePhase;
pub use template_record::TemplateRecord;
pub use template_registry::TemplateRegistry;

#[cfg(test)]
mod tests;
