use super::*;
use crate::fixtures::*;
use trogonai_proto::evaluation::bindings::v1alpha1 as v1;

fn envelope_without_event() -> EvaluationBindingEvent {
    EvaluationBindingEvent { event: None }
}

fn fold(events: &[EvaluationBindingEvent]) -> Result<EvaluationBindingState, EvaluationBindingEvolveError> {
    events.iter().try_fold(initial_state(), evolve)
}

#[test]
fn starts_missing_with_no_results() {
    assert_eq!(initial_state().status, BindingStatus::Missing);
    assert!(initial_state().recorded_results.is_empty());
}

#[test]
fn folds_bound_recorded_and_unbound() {
    let recorded = recorded_from(&record_proto("result-1", vec![]));

    let bound_state = fold(&[bound(), recorded.clone()]).unwrap();
    assert_eq!(bound_state.status, BindingStatus::Bound(binding()));
    assert!(bound_state.recorded_results.contains("result-1"));

    let unbound_state = fold(&[bound(), recorded, unbound()]).unwrap();
    assert_eq!(unbound_state.status, BindingStatus::Unbound);
}

#[test]
fn rejects_an_envelope_without_an_event() {
    assert_eq!(
        fold(&[envelope_without_event()]).unwrap_err(),
        EvaluationBindingEvolveError::MissingEvent
    );
}

#[test]
fn rejects_a_bound_event_without_a_binding() {
    let event = EvaluationBindingEvent {
        event: Some(v1::RubricBound::default().into()),
    };
    assert_eq!(
        fold(&[event]).unwrap_err(),
        EvaluationBindingEvolveError::MissingField { field: "binding" }
    );
}

#[test]
fn rejects_a_second_bound_event() {
    assert_eq!(
        fold(&[bound(), bound()]).unwrap_err(),
        EvaluationBindingEvolveError::BoundTwice
    );
    assert_eq!(
        fold(&[bound(), unbound(), bound()]).unwrap_err(),
        EvaluationBindingEvolveError::BoundTwice
    );
}

#[test]
fn rejects_an_unbound_event_without_an_active_binding() {
    let message = EvaluationBindingEvolveError::UnboundWithoutActiveBinding;
    assert_eq!(fold(&[unbound()]).unwrap_err(), message);
    assert_eq!(fold(&[bound(), unbound(), unbound()]).unwrap_err(), message);
}

#[test]
fn rejects_a_result_without_an_active_binding() {
    let recorded = recorded_from(&record_proto("result-1", vec![]));
    let message = EvaluationBindingEvolveError::RecordedWithoutActiveBinding;
    assert_eq!(fold(std::slice::from_ref(&recorded)).unwrap_err(), message);
    assert_eq!(fold(&[bound(), unbound(), recorded]).unwrap_err(), message);
}

#[test]
fn rejects_a_result_without_an_id() {
    let mut proto = record_proto("result-1", vec![]);
    proto.evaluation_result_id = Default::default();
    assert_eq!(
        fold(&[bound(), recorded_from(&proto)]).unwrap_err(),
        EvaluationBindingEvolveError::MissingField {
            field: "evaluation_result_id"
        }
    );
}

#[test]
fn rejects_a_result_recorded_twice() {
    let recorded = recorded_from(&record_proto("result-1", vec![]));
    assert_eq!(
        fold(&[bound(), recorded.clone(), recorded]).unwrap_err(),
        EvaluationBindingEvolveError::ResultRecordedTwice("result-1".to_string())
    );
}
