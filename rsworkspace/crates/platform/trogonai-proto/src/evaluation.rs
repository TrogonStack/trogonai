//! Evaluation bindings (ADR 0072): the generated types plus aliases for the oneof cases the
//! decider matches on, so callers do not spell the generated `__buffa` path.

pub mod bindings {
    pub use crate::r#gen::trogonai::evaluation::bindings::v1alpha1;

    pub use v1alpha1::__buffa::oneof::evaluation_binding_event::Event as EvaluationBindingEventCase;
    pub use v1alpha1::__buffa::oneof::finding_target::Target as FindingTargetCase;
}

pub mod rubrics {
    pub use crate::r#gen::trogonai::evaluation::rubrics::v1alpha1;
}
