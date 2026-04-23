#[path = "../common.rs"]
mod common;

use trogon_eventsourcing::testing::{TestCase, decider};

use common::{TestCommand, TestDecisionError};

fn main() {
    TestCase::new(decider::<TestCommand>())
        .then_error(TestDecisionError::AlreadyRegistered);
}
