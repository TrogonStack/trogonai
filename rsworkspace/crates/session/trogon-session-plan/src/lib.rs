mod admission;
mod decoder;
mod error;
mod lease;
mod owner;
mod shape;
mod source;
mod variables;

pub use admission::PlanAdmission;
pub use decoder::{DecodedPlan, RegisteredPlanDecoder};
pub use error::PlanError;
pub use lease::PlanAdmissionLease;
pub use owner::{
    DelegationOwnerContext, DelegationSource, PlanOwnerContext, PlanOwners, PlanSourceLease, RuntimePlanInterpreter,
};

pub mod wire {
    pub use trogonai_proto::session_contracts::trogonai::agents::agents::v1alpha1 as agent;
    pub use trogonai_proto::session_contracts::trogonai::session::plans::v1alpha1 as plan;
    pub use trogonai_proto::session_contracts::trogonai::session::sessions::v1alpha1 as session;
}
