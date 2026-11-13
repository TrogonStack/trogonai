#[cfg(stageleft_runtime)]
hydro_lang::setup!();

use hydro_lang::prelude::*;

pub mod recovery;
pub mod replicas;

pub struct Scheduler;

pub fn recover<'a>(
    advances: Stream<recovery::Input, Process<'a, Scheduler>>,
    faults: Stream<recovery::Input, Process<'a, Scheduler>>,
) -> Stream<recovery::Observation, Process<'a, Scheduler>> {
    advances
        .merge_ordered(
            faults,
            nondet!(/** a fault may occur between any recovery phases */),
        )
        .scan(
            q!(self::recovery::Recovery::default),
            q!(|state, input| Some(state.step(input))),
        )
}

pub fn competing_replicas<'a>(
    older: Stream<replicas::Worker, Process<'a, Scheduler>>,
    newer: Stream<replicas::Worker, Process<'a, Scheduler>>,
) -> Stream<replicas::Observation, Process<'a, Scheduler>> {
    older
        .merge_ordered(
            newer,
            nondet!(/** separate instances can interleave operations for one key */),
        )
        .scan(
            q!(self::replicas::Replicas::default),
            q!(|state, worker| Some(state.step(worker))),
        )
}
