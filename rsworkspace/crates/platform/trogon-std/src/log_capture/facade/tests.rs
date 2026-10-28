use super::*;

#[test]
fn tracing_falls_back_to_the_log_facade_before_subscriber_installation() {
    let Some(logs) = CapturedLogs::isolated() else {
        return;
    };
    let timeout = std::time::Duration::from_secs(7);
    tracing::info!(target: "fixture", timeout_secs = timeout.as_secs(), "connection ready");

    let records = logs.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].level, LogLevel::Info);
    assert_eq!(records[0].target, "fixture");
    assert!(records[0].message.contains("connection ready"));
    assert!(records[0].message.contains("timeout_secs=7"));
}

#[test]
fn an_isolated_test_failure_preserves_stdout_and_stderr() {
    let thread = std::thread::current();
    let name = thread.name().expect("libtest names its test threads");
    if SystemEnv.var(constants::CHILD_TEST).is_ok_and(|child| child == name) {
        println!("isolated stdout diagnostic");
        eprintln!("isolated stderr diagnostic");
        panic!("isolated test failure");
    }

    let failure = std::panic::catch_unwind(CapturedLogs::isolated)
        .err()
        .expect("the failed child must fail its parent");
    let message = failure.downcast_ref::<String>().expect("formatted failure message");
    assert!(message.contains("isolated log test failed:"));
    assert!(message.contains("isolated stdout diagnostic"));
    assert!(message.contains("isolated stderr diagnostic"));
    assert!(message.contains("isolated test failure"));
}
