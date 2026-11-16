use async_nats::jetstream::consumer::pull::{MessagesError, MessagesErrorKind};
use async_nats::jetstream::context::{ConsumerInfoError, ConsumerInfoErrorKind};

use super::InvalidationLoss;

#[test]
fn records_each_loss_under_its_own_reason() {
    let losses = [
        InvalidationLoss::Failed(MessagesError::new(MessagesErrorKind::MissingHeartbeat)),
        InvalidationLoss::Gap {
            expected: 2,
            arrived: 3,
        },
        InvalidationLoss::Unplaced("fake-no-delivery-info".into()),
        InvalidationLoss::Ended,
        InvalidationLoss::Missing {
            received: 1,
            delivered: 2,
        },
        InvalidationLoss::PositionUnknown(ConsumerInfoError::new(ConsumerInfoErrorKind::TimedOut)),
    ];

    let reasons: Vec<_> = losses.iter().map(|loss| loss.reason().as_str()).collect();

    assert_eq!(
        reasons,
        ["failed", "gap", "unplaced", "ended", "missing", "position_unknown"]
    );
}

#[test]
fn says_which_notice_arrived_out_of_turn() {
    let loss = InvalidationLoss::Gap {
        expected: 2,
        arrived: 3,
    };

    assert_eq!(loss.to_string(), "notice 3 arrived where notice 2 was due");
}

#[test]
fn says_how_many_delivered_notices_never_arrived() {
    let loss = InvalidationLoss::Missing {
        received: 1,
        delivered: 2,
    };

    assert_eq!(
        loss.to_string(),
        "notices through 2 were delivered, but only through 1 arrived"
    );
}

#[test]
fn says_why_a_notice_could_not_be_placed() {
    let loss = InvalidationLoss::Unplaced("fake-no-delivery-info".into());

    assert_eq!(
        loss.to_string(),
        "a notice arrived without its place in the delivery: fake-no-delivery-info"
    );
}

#[test]
fn says_the_consumer_stopped_delivering() {
    assert_eq!(
        InvalidationLoss::Ended.to_string(),
        "the invalidation consumer stopped delivering notices"
    );
}
