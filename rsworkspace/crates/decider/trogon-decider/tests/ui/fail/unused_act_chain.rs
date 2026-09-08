#![deny(unused_must_use)]

#[path = "../common.rs"]
mod common;

use common::{TestCommand, TestEvent};
use trogon_decider::Decision;

fn main() {
    Decision::<TestCommand>::act().execute(|_, _| Decision::event(TestEvent::Registered));
}
