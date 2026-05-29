#[path = "../common.rs"]
mod common;

use trogon_decider::testing::TestCase;

use common::TestCommand;

fn main() {
    TestCase::<TestCommand>::new().when(TestCommand);
}
