use super::*;

fn names(checks: Vec<(&'static str, CheckFn)>) -> Vec<&'static str> {
    checks.into_iter().map(|(name, _)| name).collect()
}

#[test]
fn every_check_is_listed_once_by_name() {
    let all = names(checks());
    let mut unique = all.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(all.len(), unique.len());
    assert!(all.contains(&"denied_connect_fails"));
    assert!(all.contains(&"file_archive_round_trips_a_tree"));
}

#[test]
fn a_worker_without_commands_gets_fewer_checks_than_a_shell_worker() {
    assert!(checks_for(ExecModel::None).len() < checks_for(ExecModel::Shell).len());
    assert!(checks_for(ExecModel::JsEval).len() < checks().len());
}
