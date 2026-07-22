#![deny(unused_must_use)]

#[path = "../common.rs"]
mod common;

use common::TestCommand;
use trogon_decider::testing::TestCase;

fn main() {
    TestCase::<TestCommand>::new().given_no_history();
}
