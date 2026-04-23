#[path = "../common.rs"]
mod common;

use trogon_eventsourcing::testing::{TestCase, decider};

use common::{TestCommand, TestDecisionError, TestEvent};

fn main() {
    TestCase::new(decider::<TestCommand>())
        .given([TestEvent::Registered])
        .when(TestCommand)
        .then_error(TestDecisionError::AlreadyRegistered);
}
