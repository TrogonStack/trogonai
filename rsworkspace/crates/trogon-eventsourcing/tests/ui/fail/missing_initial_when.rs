#[path = "../common.rs"]
mod common;

use trogon_eventsourcing::testing::{TestCase, decider, expect_error};

use common::{TestCommand, TestDecisionError};

fn main() {
    TestCase::new(decider::<TestCommand>())
        .then(expect_error(TestDecisionError::AlreadyRegistered));
}
