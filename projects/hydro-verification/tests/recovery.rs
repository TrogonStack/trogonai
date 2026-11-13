use std::sync::atomic::{AtomicBool, Ordering};

use hydro_lang::prelude::*;
use trogonai_hydro_verification::recovery::{Fault, Input, Observation, Protocol};
use trogonai_hydro_verification::replicas::Worker;
use trogonai_hydro_verification::{Scheduler, competing_replicas, recover};

const ADVANCES: usize = 28;

fn checkpoint_precedes_effect(observations: &[Observation]) -> Option<&Observation> {
    let mut previous = 0;
    observations.iter().find(|observation| {
        let changed = observation.checkpoint > previous;
        previous = observation.checkpoint;
        changed && observation.scheduled != (observation.checkpoint != 2)
    })
}

#[test]
fn serial_recovery_explores_crash_and_response_loss_boundaries() {
    let mut flow = FlowBuilder::new();
    let process = flow.process::<Scheduler>();
    let (advance, advances) = process.sim_input();
    let (inject, faults) = process.sim_input();
    let output = recover(advances, faults).sim_output();
    let simulator = flow.sim().compiled();

    for fault in [
        None,
        Some(Fault::Crash),
        Some(Fault::RejectPublish),
        Some(Fault::LosePublishResponse),
        Some(Fault::RejectCheckpoint),
        Some(Fault::LoseAck),
    ] {
        let effect_response_loss_observed = AtomicBool::new(false);
        let publish_rejection_observed = AtomicBool::new(false);
        let executions = simulator.exhaustive(async || {
            advance.send_many(std::iter::repeat_n(
                Input::Advance(Protocol::DurableBeforeAck),
                ADVANCES,
            ));
            if let Some(fault) = fault {
                inject.send(Input::Inject(fault));
            }
            let observations: Vec<Observation> = output.collect().await;
            assert!(!observations.is_empty());
            for observation in &observations {
                assert!(
                    observation.acknowledged <= observation.checkpoint,
                    "{observation:?}"
                );
                effect_response_loss_observed
                    .fetch_or(observation.accepted_without_response > 0, Ordering::Relaxed);
                publish_rejection_observed
                    .fetch_or(observation.rejected_publishes > 0, Ordering::Relaxed);
            }
            assert!(checkpoint_precedes_effect(&observations).is_none());
            let final_state = &observations[observations.len() - 1];
            assert_eq!(final_state.checkpoint, 3, "{final_state:?}");
            assert_eq!(final_state.acknowledged, 3, "{final_state:?}");
            assert!(final_state.scheduled, "{final_state:?}");
        });
        eprintln!("serial fault={fault:?}: {executions} Hydro executions");
        assert!(executions > usize::from(fault.is_some()));
        if fault == Some(Fault::LosePublishResponse) {
            assert!(effect_response_loss_observed.load(Ordering::Relaxed));
        }
        if fault == Some(Fault::RejectPublish) {
            assert!(publish_rejection_observed.load(Ordering::Relaxed));
        }
    }
}

#[test]
fn negative_controls_detect_ack_and_checkpoint_reordering() {
    let mut flow = FlowBuilder::new();
    let process = flow.process::<Scheduler>();
    let (advance, advances) = process.sim_input();
    let (inject, faults) = process.sim_input();
    let output = recover(advances, faults).sim_output();
    let simulator = flow.sim().compiled();

    for protocol in [
        Protocol::AckBeforeCheckpoint,
        Protocol::CheckpointBeforeEffect,
    ] {
        let detected = AtomicBool::new(false);
        let executions = simulator.exhaustive(async || {
            advance.send_many(std::iter::repeat_n(Input::Advance(protocol), ADVANCES));
            inject.send(Input::Inject(Fault::Crash));
            let observations: Vec<Observation> = output.collect().await;
            let witness = match protocol {
                Protocol::AckBeforeCheckpoint => observations
                    .iter()
                    .find(|state| state.acknowledged > state.checkpoint),
                Protocol::CheckpointBeforeEffect => checkpoint_precedes_effect(&observations),
                Protocol::DurableBeforeAck => unreachable!(),
            };
            if let Some(witness) = witness
                && !detected.swap(true, Ordering::Relaxed)
            {
                eprintln!("detected {protocol:?}: {witness:?}");
            }
        });
        assert!(executions > 1);
        assert!(
            detected.load(Ordering::Relaxed),
            "negative control was not detected: {protocol:?}"
        );
    }
}

#[test]
fn negative_control_exposes_cross_instance_pause_resume_race() {
    let mut flow = FlowBuilder::new();
    let process = flow.process::<Scheduler>();
    let (pause, pauses) = process.sim_input();
    let (resume, resumes) = process.sim_input();
    let output = competing_replicas(pauses, resumes).sim_output();
    let detected = AtomicBool::new(false);
    let executions = flow.sim().exhaustive(async || {
        pause.send_many([Worker::OlderPause; 4]);
        resume.send_many([Worker::NewerResume; 5]);
        let observations: Vec<trogonai_hydro_verification::replicas::Observation> =
            output.collect().await;
        for observation in observations {
            if observation.pause_acknowledged
                && observation.resume_acknowledged
                && observation.checkpoint == 3
                && !observation.scheduled
                && !detected.swap(true, Ordering::Relaxed)
            {
                eprintln!("cross-instance counterexample: {observation:?}");
            }
        }
    });
    eprintln!("cross-instance control: {executions} Hydro executions");
    assert!(executions > 1);
    assert!(detected.load(Ordering::Relaxed));
}
