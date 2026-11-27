use super::*;

fn lease(n: &str) -> LeaseId {
    LeaseId::new(n)
}

#[test]
fn each_intent_moves_only_from_the_states_that_allow_it() {
    let a = lease("a");
    let running = EntityState::Running { lease: a.clone() };
    let paused = EntityState::Paused { lease: a.clone() };
    let stopping = EntityState::Stopping { lease: a.clone() };

    assert!(matches!(
        event_for(&running, Intent::Stop, Some(&a)),
        Ok(Some(EntityEvent::StopRequested { .. }))
    ));
    assert!(matches!(
        event_for(&paused, Intent::Stop, Some(&a)),
        Ok(Some(EntityEvent::StopRequested { .. }))
    ));
    assert_eq!(event_for(&stopping, Intent::Stop, Some(&a)), Ok(None));
    assert!(matches!(
        event_for(&running, Intent::Pause, Some(&a)),
        Ok(Some(EntityEvent::Paused { .. }))
    ));
    assert_eq!(event_for(&paused, Intent::Pause, Some(&a)), Ok(None));
    assert!(matches!(
        event_for(&paused, Intent::Resume, Some(&a)),
        Ok(Some(EntityEvent::Resumed { .. }))
    ));
    assert_eq!(event_for(&running, Intent::Resume, Some(&a)), Ok(None));
    assert!(matches!(
        event_for(&running, Intent::Delete, Some(&a)),
        Ok(Some(EntityEvent::DeleteRequested))
    ));
    assert_eq!(event_for(&EntityState::Deleting, Intent::Delete, Some(&a)), Ok(None));
}

#[test]
fn a_verb_for_another_lease_does_not_apply() {
    let running = EntityState::Running { lease: lease("a") };
    for intent in [Intent::Stop, Intent::Pause, Intent::Resume] {
        assert_eq!(event_for(&running, intent, Some(&lease("b"))), Err(()));
    }
    assert_eq!(
        event_for(&EntityState::Stopped, Intent::Pause, Some(&lease("a"))),
        Err(())
    );
}
