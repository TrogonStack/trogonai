#[path = "../common.rs"]
mod common;

use trogon_decider::testing::TestCase;

use common::{TestCommand, TestDecisionError, TestEvent};

fn main() {
    TestCase::<TestCommand>::new()
        .given([TestEvent::Registered])
        .when(TestCommand)
        .then_error(TestDecisionError::AlreadyRegistered);
}
