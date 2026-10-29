use super::super::constants::SLACK_ACK_DEADLINE;
use super::*;

fn secs(value: u64) -> NonZeroDuration {
    NonZeroDuration::from_secs(value).unwrap()
}

#[test]
fn default_budget_leaves_room_inside_the_slack_deadline() {
    let budget = AckBudget::default();
    assert_eq!(budget.limit(), Duration::from_millis(2800));
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
        assert_eq!(budget.limit(), Duration::from_millis(2800));
        assert_eq!(budget.capped_from(), Some(secs(requested)));
    }
}

#[test]
fn a_timeout_of_two_seconds_still_fits() {
    let budget = AckBudget::new(Some(secs(2)));
    assert_eq!(budget.limit(), Duration::from_secs(2));
    assert_eq!(budget.capped_from(), None);
}

#[test]
fn a_delivery_that_awaits_a_reply_publishes_within_one_second() {
    let budget = AckBudget::default();
    assert_eq!(budget.publish_limit(AckPlan::AwaitReply), Duration::from_secs(1));
    assert_eq!(budget.publish_limit(AckPlan::Acknowledge), Duration::from_millis(2800));
}

#[test]
fn option_lookups_are_requests_with_no_publish() {
    assert_eq!(AckPlan::for_interaction("block_suggestion"), AckPlan::RequestOnly);
    assert_eq!(
        AckBudget::default().publish_limit(AckPlan::RequestOnly),
        Duration::from_millis(2800)
    );
}

#[test]
fn a_shorter_configured_limit_still_wins_for_a_reply() {
    let budget = AckBudget::new(Some(NonZeroDuration::from_millis(400).unwrap()));
    assert_eq!(budget.publish_limit(AckPlan::AwaitReply), Duration::from_millis(400));
}

#[test]
fn only_modal_submissions_await_a_reply_among_interactions() {
    assert_eq!(AckPlan::for_interaction("view_submission"), AckPlan::AwaitReply);
    for other in ["block_actions", "view_closed", "shortcut"] {
        assert_eq!(AckPlan::for_interaction(other), AckPlan::Acknowledge);
    }
}

#[test]
fn the_reply_gets_what_the_publish_left_of_the_ack_limit() {
    assert_eq!(
        AckBudget::reply_limit(Duration::from_millis(300)),
        ACK_LIMIT_MAX - Duration::from_millis(300)
    );
    assert_eq!(AckBudget::reply_limit(ACK_LIMIT_MAX * 2), Duration::ZERO);
}
