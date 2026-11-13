#[cfg(stageleft_runtime)]
hydro_lang::setup!();

use hydro_lang::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Command {
    Create,
    Pause,
    Resume,
    Remove,
}

pub struct Scheduler;

pub fn client_orders<'a>(
    left: Stream<Command, Process<'a, Scheduler>>,
    right: Stream<Command, Process<'a, Scheduler>>,
) -> Stream<Command, Process<'a, Scheduler>> {
    left.merge_ordered(
        right,
        nondet!(/** client command streams can interleave while preserving each client's order */),
    )
}
