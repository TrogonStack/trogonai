use super::*;

fn pool(value: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(LABEL_WARM_POOL.to_owned(), value.to_owned())])
}

#[test]
fn a_pod_outside_the_pool_has_no_slot() {
    assert_eq!(WarmSlotState::of(None, Some("Running"), false), None);
    assert_eq!(WarmSlotState::of(Some(&BTreeMap::new()), Some("Running"), false), None);
}

#[test]
fn a_deleting_pod_is_terminating() {
    assert_eq!(
        WarmSlotState::of(Some(&pool(WARM_POOL_IDLE)), Some("Running"), true),
        Some(WarmSlotState::Terminating)
    );
}

#[test]
fn an_unknown_pool_label_has_no_slot() {
    assert_eq!(WarmSlotState::of(Some(&pool("mystery")), Some("Running"), false), None);
}

#[test]
fn an_idle_pod_follows_its_phase() {
    let labels = pool(WARM_POOL_IDLE);
    assert_eq!(
        WarmSlotState::of(Some(&labels), Some(POD_PHASE_RUNNING), false),
        Some(WarmSlotState::Ready)
    );
    assert_eq!(
        WarmSlotState::of(Some(&labels), Some(POD_PHASE_FAILED), false),
        Some(WarmSlotState::Failed)
    );
    assert_eq!(
        WarmSlotState::of(Some(&labels), Some(POD_PHASE_SUCCEEDED), false),
        Some(WarmSlotState::Failed)
    );
    assert_eq!(
        WarmSlotState::of(Some(&labels), Some("Pending"), false),
        Some(WarmSlotState::Warming)
    );
    assert_eq!(
        WarmSlotState::of(Some(&labels), None, false),
        Some(WarmSlotState::Warming)
    );
}

#[test]
fn a_claimed_pod_is_claimed() {
    assert_eq!(
        WarmSlotState::of(Some(&pool(WARM_POOL_CLAIMED)), None, false),
        Some(WarmSlotState::Claimed)
    );
}
