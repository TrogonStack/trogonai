#[path = "../common.rs"]
mod common;

use trogon_eventsourcing::testing::{TestCase, decider};

use common::{TestCommand, TestDecisionError};

fn main() {
    TestCase::new(decider::<TestCommand>())
        .given_no_history()
        .then_error(TestDecisionError::AlreadyRegistered);
}
