#[path = "../common.rs"]
mod common;

use trogon_decider::testing::TestCase;

use common::{TestCommand, TestEvent};

fn main() {
    TestCase::<TestCommand>::new()
        .given_no_history()
        .when(TestCommand)
        .then([TestEvent::Registered]);
}
