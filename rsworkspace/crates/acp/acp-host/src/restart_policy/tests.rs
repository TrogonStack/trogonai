use super::*;

const SHORT: Duration = Duration::from_millis(10);

#[test]
fn the_first_restart_waits_the_initial_delay() {
    assert_eq!(RestartPolicy::DEFAULT.next_delay(None, SHORT), Duration::from_secs(1));
}

#[test]
fn back_to_back_crashes_double_the_delay_up_to_the_cap() {
    let policy = RestartPolicy::DEFAULT;
    let mut delay = None;
    let mut seen = Vec::new();
    for _ in 0..8 {
        let next = policy.next_delay(delay, SHORT);
        seen.push(next.as_secs());
        delay = Some(next);
    }
    assert_eq!(seen, [1, 2, 4, 8, 16, 30, 30, 30]);
}

#[test]
fn a_run_that_stayed_up_resets_the_delay() {
    let policy = RestartPolicy::DEFAULT;
    let next = policy.next_delay(Some(Duration::from_secs(30)), Duration::from_secs(120));
    assert_eq!(next, Duration::from_secs(1));
}
