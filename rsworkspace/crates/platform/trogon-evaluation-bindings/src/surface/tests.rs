use trogonai_proto::evaluation::bindings::v1alpha1::{Finding, FindingTarget};

use super::*;
use crate::fixtures::*;

#[test]
fn a_finding_without_a_target_is_never_inside_the_surface() {
    let untargeted = Finding {
        target: Default::default(),
        ..settings_finding("instructions")
    };
    assert!(!finding_inside_surface(&untargeted, &surface(&["instructions"], &[])));
}

#[test]
fn a_finding_with_an_unset_target_oneof_is_never_inside_the_surface() {
    let unset = Finding {
        target: FindingTarget { target: None }.into(),
        ..settings_finding("instructions")
    };
    assert!(!finding_inside_surface(&unset, &surface(&["instructions"], &[])));
}
