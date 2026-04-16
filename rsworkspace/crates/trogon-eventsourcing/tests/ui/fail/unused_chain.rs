#![deny(unused_must_use)]

#[path = "../common.rs"]
mod common;

use common::TestCommand;
use trogon_eventsourcing::testing::{TestCase, decider};

fn main() {
    TestCase::new(decider::<TestCommand>()).given([]);
}
