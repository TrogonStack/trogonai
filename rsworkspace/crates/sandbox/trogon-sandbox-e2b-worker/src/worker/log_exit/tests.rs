use super::*;

#[test]
fn a_failed_task_is_logged_and_a_clean_one_is_not() {
    log_exit("test task", Err::<(), _>("it broke"));
    log_exit("test task", Ok::<(), &str>(()));
}
