#[path = "../common.rs"]
mod common;

use trogon_decider::testing::TestCase;

use common::{TestCommand, TestEvent, TestState};

fn main() {
    TestCase::<TestCommand>::new()
        .given_state(TestState::Missing)
        .given([TestEvent::Registered]);
}
