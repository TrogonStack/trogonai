#[path = "../common.rs"]
mod common;

use trogon_eventsourcing::testing::{TestCase, decider, expect_error};

use common::{TestCommand, TestDecisionError, TestEvent};

fn main() {
    TestCase::new(decider::<TestCommand>())
        .given([TestEvent::Registered])
        .when(TestCommand)
        .then(expect_error(TestDecisionError::AlreadyRegistered));
}
