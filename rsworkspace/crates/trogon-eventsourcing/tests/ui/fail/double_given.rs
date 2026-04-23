#[path = "../common.rs"]
mod common;

use trogon_eventsourcing::testing::{TestCase, decider};

use common::TestCommand;

fn main() {
    TestCase::new(decider::<TestCommand>())
        .given_no_history()
        .given([]);
}
