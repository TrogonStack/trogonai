use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn live(status: LeaseStatus) -> Observed {
    Observed::Live {
        status,
        freezing: false,
        auto_paused: false,
    }
}

#[test]
fn a_delete_is_always_finished() {
    assert_eq!(plan_step(&EntityState::Deleting, &Observed::Gone), Step::FinishDelete);
}

#[test]
fn a_stop_releases_a_live_lease_and_ends_the_run_of_a_gone_one() {
    let stopping = EntityState::Stopping { lease: lease() };
    assert_eq!(
        plan_step(&stopping, &live(LeaseStatus::Running)),
        Step::ReleaseLease(lease())
    );
    assert_eq!(plan_step(&stopping, &Observed::Gone), Step::EndRun(lease()));
}

#[test]
fn a_pause_runs_only_against_a_running_lease() {
    let paused = EntityState::Paused { lease: lease() };
    assert_eq!(
        plan_step(&paused, &live(LeaseStatus::Running)),
        Step::PauseLease(lease())
    );
    assert_eq!(plan_step(&paused, &live(LeaseStatus::Paused)), Step::Nothing);
    assert_eq!(plan_step(&paused, &live(LeaseStatus::Suspending)), Step::Nothing);
}

#[test]
fn a_running_entity_over_a_user_paused_lease_is_resumed_but_not_an_idle_pause() {
    let running = EntityState::Running { lease: lease() };
    assert_eq!(
        plan_step(&running, &live(LeaseStatus::Paused)),
        Step::ResumeLease(lease())
    );
    let idle = Observed::Live {
        status: LeaseStatus::Paused,
        freezing: false,
        auto_paused: true,
    };
    assert_eq!(plan_step(&running, &idle), Step::Nothing);
}

#[test]
fn a_freeze_in_flight_is_left_to_its_owner() {
    let freezing = Observed::Live {
        status: LeaseStatus::Running,
        freezing: true,
        auto_paused: false,
    };
    assert_eq!(
        plan_step(&EntityState::Paused { lease: lease() }, &freezing),
        Step::Nothing
    );
    assert_eq!(
        plan_step(&EntityState::Stopping { lease: lease() }, &freezing),
        Step::Nothing
    );
}

#[test]
fn states_reached_by_observation_are_not_acted_on() -> TestResult {
    for state in [
        EntityState::Creating,
        EntityState::Stopped,
        EntityState::Starting { lease: lease() },
        EntityState::Running { lease: lease() },
        EntityState::Failed { reason: "x".into() },
    ] {
        assert_eq!(plan_step(&state, &Observed::Gone), Step::Nothing, "{state:?}");
        assert_eq!(
            plan_step(&state, &live(LeaseStatus::Running)),
            Step::Nothing,
            "{state:?}"
        );
    }
    Ok(())
}
