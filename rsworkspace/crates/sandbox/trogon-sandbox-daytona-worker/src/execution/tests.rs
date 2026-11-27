use super::*;

use crate::constants::MAX_COMPLETED_RESULTS;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn execution(id: &str, completed_at: Option<Instant>) -> Execution {
    Execution {
        lease_id: "lease-1".to_owned(),
        sandbox_id: "sbx-1".to_owned(),
        session_id: format!("trogon-{id}"),
        command_id: "cmd".to_owned(),
        files: SideFiles::new(id),
        max_output_bytes: 1024,
        has_deadline: false,
        stdin: None,
        ordinary: None,
        completed: completed_at.map(|_| CommandOutput::default()),
        completed_at,
    }
}

fn kept(executions: &HashMap<String, Execution>) -> Vec<String> {
    let mut ids: Vec<String> = executions.keys().cloned().collect();
    ids.sort();
    ids
}

#[test]
fn a_finished_result_older_than_the_ttl_is_dropped() -> TestResult {
    let now = Instant::now();
    let old = now.checked_sub(Duration::from_secs(700)).ok_or("clock too young")?;
    let fresh = now.checked_sub(Duration::from_secs(5)).ok_or("clock too young")?;
    let mut executions = HashMap::from([
        ("old".to_owned(), execution("old", Some(old))),
        ("fresh".to_owned(), execution("fresh", Some(fresh))),
    ]);
    evict_completed(&mut executions, now, Duration::from_secs(600), 256);
    assert_eq!(kept(&executions), ["fresh"]);
    Ok(())
}

#[test]
fn a_running_command_is_never_evicted() -> TestResult {
    let now = Instant::now();
    let mut executions = HashMap::from([("running".to_owned(), execution("running", None))]);
    evict_completed(
        &mut executions,
        now + Duration::from_secs(100_000),
        Duration::from_secs(1),
        0,
    );
    assert_eq!(kept(&executions), ["running"]);
    Ok(())
}

#[test]
fn the_oldest_finished_results_go_first_beyond_the_cap() -> TestResult {
    let now = Instant::now();
    let mut executions = HashMap::new();
    for age in 1..=5u64 {
        let at = now.checked_sub(Duration::from_secs(age)).ok_or("clock too young")?;
        let id = format!("run-{age}");
        executions.insert(id.clone(), execution(&id, Some(at)));
    }
    executions.insert("running".to_owned(), execution("running", None));
    evict_completed(&mut executions, now, Duration::from_secs(600), 2);
    assert_eq!(kept(&executions), ["run-1", "run-2", "running"]);
    Ok(())
}

#[test]
fn the_cap_matches_what_a_lease_may_keep() {
    assert_eq!(MAX_COMPLETED_RESULTS, 256);
}
