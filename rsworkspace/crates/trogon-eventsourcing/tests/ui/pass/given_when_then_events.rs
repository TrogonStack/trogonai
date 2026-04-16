#[path = "../common.rs"]
mod common;

use trogon_eventsourcing::testing::{TestCase, decider};

use common::{TestCommand, TestEvent};

fn main() {
    TestCase::new(decider::<TestCommand>())
        .given([])
        .when(TestCommand)
        .then([TestEvent::Registered]);
}
