use trogon_decider::testing::TestCase;

use super::*;
use crate::fixtures::*;
use trogonai_proto::evaluation::bindings::v1alpha1 as v1;

#[test]
fn unbinds_an_active_binding() {
    TestCase::<UnbindRubric>::new()
        .given([bound()])
        .when(unbind_command())
        .then([unbound()]);
}

#[test]
fn unbinds_a_binding_that_already_recorded_results() {
    let proto = record_proto("result-1", vec![]);

    TestCase::<UnbindRubric>::new()
        .given([bound(), recorded_from(&proto)])
        .when(unbind_command())
        .then([unbound()]);
}

#[test]
fn refuses_a_binding_that_does_not_exist() {
    TestCase::<UnbindRubric>::new()
        .given_no_history()
        .when(unbind_command())
        .then_error(EvaluationBindingDecideError::NotFound);
}

#[test]
fn refuses_a_binding_that_is_already_unbound() {
    TestCase::<UnbindRubric>::new()
        .given([bound(), unbound()])
        .when(unbind_command())
        .then_error(EvaluationBindingDecideError::NotFound);
}

#[test]
fn rejects_a_command_without_a_binding_id() {
    let missing = UnbindRubric::new(v1::UnbindRubric::default());
    assert_eq!(missing.unwrap_err().field, "binding_id");

    let empty = UnbindRubric::new(v1::UnbindRubric {
        binding_id: v1::EvaluationBindingId { value: String::new() }.into(),
    });
    assert_eq!(empty.unwrap_err().field, "binding_id");
}

#[test]
fn targets_the_binding_stream_and_requires_it_to_exist_unchanged() {
    let command = unbind_command();

    assert_eq!(command.binding_id(), BINDING_ID);
    assert_eq!(UnbindRubric::stream_id(&command), BINDING_ID);
    assert_eq!(UnbindRubric::WRITE_PRECONDITION, WritePrecondition::StreamUnchanged);
}

#[test]
fn not_found_displays_the_fault_message() {
    assert_eq!(
        EvaluationBindingDecideError::NotFound.to_string(),
        "The evaluation binding does not exist or is unbound."
    );
}
