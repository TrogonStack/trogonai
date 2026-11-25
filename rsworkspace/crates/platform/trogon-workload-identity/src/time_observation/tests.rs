use std::cell::Cell;
use std::time::Duration;

use super::*;

#[test]
fn delayed_wall_clock_read_does_not_restart_a_finite_source_lifetime() {
    let began = Cell::new(None);
    let sample = Cell::new(None);
    let observation = TimeObservation::with_clock(|| {
        began.set(Some(Instant::now()));
        let wall = SystemTime::now();
        sample.set(Some(wall));
        std::thread::sleep(Duration::from_millis(20));
        wall
    });
    let lifetime = Duration::from_secs(60);
    let deadline = observation.deadline(sample.get().unwrap() + lifetime).unwrap();
    assert!(
        deadline <= began.get().unwrap() + lifetime,
        "clock-read delay must consume authority instead of restarting it"
    );
}

#[test]
fn an_expiry_already_in_the_past_has_no_deadline() {
    let observation = TimeObservation::now();
    assert!(
        observation
            .deadline(observation.wall() - Duration::from_secs(1))
            .is_err()
    );
    assert!(
        observation
            .deadline(observation.wall() + Duration::from_secs(1))
            .is_ok()
    );
}
