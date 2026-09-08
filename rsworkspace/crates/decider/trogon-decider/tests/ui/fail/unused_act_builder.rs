#![deny(unused_must_use)]

#[path = "../common.rs"]
mod common;

use common::TestCommand;
use trogon_decider::Decision;

fn main() {
    Decision::<TestCommand>::act();
}
