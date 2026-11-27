use serde_json::json;
use trogon_sandbox_contract::{EntityState, EntityStateKind, EntityStateKind as K, LeaseId, SandboxEntityRecord};
use trogon_sandbox_protocol::{
    EntityEvent, EntityEvent as E, IllegalTransitionError, apply_entity_event, next_entity_state,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn lease(name: &str) -> LeaseId {
    LeaseId::new(name)
}

fn states() -> Vec<EntityState> {
    vec![
        EntityState::Creating,
        EntityState::Starting { lease: lease("a") },
        EntityState::Running { lease: lease("a") },
        EntityState::Paused { lease: lease("a") },
        EntityState::Stopping { lease: lease("a") },
        EntityState::Stopped,
        EntityState::Deleting,
        EntityState::Failed { reason: "x".into() },
    ]
}

fn events(l: &LeaseId) -> Vec<EntityEvent> {
    vec![
        EntityEvent::Provisioned,
        EntityEvent::StartRequested { lease: l.clone() },
        EntityEvent::LeaseReady { lease: l.clone() },
        EntityEvent::Paused { lease: l.clone() },
        EntityEvent::Resumed { lease: l.clone() },
        EntityEvent::StopRequested { lease: l.clone() },
        EntityEvent::RunEnded { lease: l.clone() },
        EntityEvent::DeleteRequested,
        EntityEvent::Failed { reason: "r".into() },
        EntityEvent::Recovered,
    ]
}

/// An independent statement of every legal move when the lease matches.
fn expected(state: &EntityState, event: &EntityEvent) -> Option<EntityStateKind> {
    match (state.kind(), event) {
        (K::Creating, E::Provisioned) => Some(K::Stopped),
        (K::Stopped | K::Failed, E::StartRequested { .. }) => Some(K::Starting),
        (K::Starting, E::LeaseReady { .. }) => Some(K::Running),
        (K::Running, E::Paused { .. }) => Some(K::Paused),
        (K::Paused, E::Resumed { .. }) => Some(K::Running),
        (K::Starting | K::Running | K::Paused, E::StopRequested { .. }) => Some(K::Stopping),
        (K::Starting | K::Running | K::Paused | K::Stopping, E::RunEnded { .. }) => Some(K::Stopped),
        (
            K::Creating | K::Stopped | K::Failed | K::Starting | K::Running | K::Paused | K::Stopping,
            E::DeleteRequested,
        ) => Some(K::Deleting),
        (K::Creating | K::Starting | K::Running | K::Paused | K::Stopping, E::Failed { .. }) => Some(K::Failed),
        (K::Failed, E::Recovered) => Some(K::Stopped),
        _ => None,
    }
}

fn record() -> Result<SandboxEntityRecord, serde_json::Error> {
    serde_json::from_value(json!({
        "entity_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "tenant": "acme",
        "created_at": { "secs_since_epoch": 1, "nanos_since_epoch": 0 },
        "spec": {
            "image": { "kind": "registry", "reference": { "registry": "ghcr.io", "repository": "acme/app" } },
            "workspace": { "empty": {} },
            "cpu_seconds": 10,
            "memory_mib": 512,
        },
        "persistence_tag": { "tenant": "acme", "name": "persist" },
        "state": "creating",
    }))
}

#[test]
fn every_state_event_pair_matches_the_table() -> TestResult {
    let mut legal = 0;
    for state in states() {
        for event in events(&lease("a")) {
            let got = next_entity_state(&state, &event);
            match expected(&state, &event) {
                Some(kind) => {
                    legal += 1;
                    assert_eq!(got?.kind(), kind, "{state:?} {event:?}");
                }
                None => {
                    let error = got.err().ok_or("expected an illegal transition")?;
                    assert_eq!(
                        error,
                        IllegalTransitionError {
                            from: state.kind(),
                            event: event.name()
                        }
                    );
                }
            }
        }
    }
    assert_eq!(legal, 26);
    Ok(())
}

#[test]
fn lease_bound_events_reject_a_different_lease() {
    for state in states().into_iter().filter(|s| s.lease().is_some()) {
        for event in events(&lease("b")) {
            let lease_event = matches!(
                event,
                EntityEvent::LeaseReady { .. }
                    | EntityEvent::Paused { .. }
                    | EntityEvent::Resumed { .. }
                    | EntityEvent::StopRequested { .. }
                    | EntityEvent::RunEnded { .. }
            );
            if lease_event {
                assert!(next_entity_state(&state, &event).is_err(), "{state:?} {event:?}");
            }
        }
    }
}

#[test]
fn deleting_is_terminal() {
    for event in events(&lease("a")) {
        assert!(next_entity_state(&EntityState::Deleting, &event).is_err());
    }
}

#[test]
fn the_error_names_the_state_and_the_event() -> TestResult {
    let error = next_entity_state(&EntityState::Stopped, &EntityEvent::Recovered)
        .err()
        .ok_or("expected an error")?;
    assert_eq!(error.to_string(), "illegal entity transition from Stopped on recovered");
    Ok(())
}

#[test]
fn a_delete_keeps_the_bound_lease_for_whoever_finishes_it() -> TestResult {
    let mut record = record()?;
    apply_entity_event(&mut record, &EntityEvent::Provisioned)?;
    apply_entity_event(&mut record, &EntityEvent::StartRequested { lease: lease("a") })?;
    apply_entity_event(&mut record, &EntityEvent::DeleteRequested)?;
    assert_eq!(record.state, EntityState::Deleting);
    assert_eq!(record.current_lease, Some(lease("a")));
    Ok(())
}

#[test]
fn apply_tracks_lease_and_generation() -> TestResult {
    let mut record = record()?;
    apply_entity_event(&mut record, &EntityEvent::Provisioned)?;
    apply_entity_event(&mut record, &EntityEvent::StartRequested { lease: lease("a") })?;
    assert_eq!(record.generation, 1);
    assert_eq!(record.current_lease, Some(lease("a")));
    apply_entity_event(&mut record, &EntityEvent::RunEnded { lease: lease("a") })?;
    assert_eq!(record.current_lease, None);
    assert!(apply_entity_event(&mut record, &EntityEvent::Recovered).is_err());
    assert_eq!(record.state, EntityState::Stopped);
    assert_eq!(record.generation, 1);
    Ok(())
}
