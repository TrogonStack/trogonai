#[path = "../common.rs"]
mod common;

use trogon_decider::testing::TestCase;

use common::{TestCommand, TestDecisionError};

fn main() {
    TestCase::<TestCommand>::new()
        .then_error(TestDecisionError::AlreadyRegistered);
}
