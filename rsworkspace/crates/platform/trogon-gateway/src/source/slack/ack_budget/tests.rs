use super::super::constants::SLACK_ACK_DEADLINE;
use super::*;

fn secs(value: u64) -> NonZeroDuration {
    NonZeroDuration::from_secs(value).unwrap()
}

#[test]
fn default_budget_leaves_room_inside_the_slack_deadline() {
    let budget = AckBudget::default();
    assert_eq!(budget.limit(), Duration::from_millis(2500));
    assert!(budget.limit() < SLACK_ACK_DEADLINE);
    assert_eq!(budget.capped_from(), None);
}

#[test]
fn configured_timeouts_inside_the_budget_are_kept() {
    let budget = AckBudget::new(Some(secs(1)));
    assert_eq!(budget.limit(), Duration::from_secs(1));
    assert_eq!(budget.capped_from(), None);
}

#[test]
fn configured_timeouts_beyond_the_budget_are_capped_and_remembered() {
    for requested in [3, 10, 60] {
        let budget = AckBudget::new(Some(secs(requested)));
        assert_eq!(budget.limit(), Duration::from_millis(2500));
        assert_eq!(budget.capped_from(), Some(secs(requested)));
    }
}

#[test]
fn a_timeout_of_two_seconds_still_fits() {
    let budget = AckBudget::new(Some(secs(2)));
    assert_eq!(budget.limit(), Duration::from_secs(2));
    assert_eq!(budget.capped_from(), None);
}
