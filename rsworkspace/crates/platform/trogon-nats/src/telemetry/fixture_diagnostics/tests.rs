use super::*;
use serde_json::{Value, json};
use testcontainers_modules::testcontainers::core::Mount;

mod docker_tests;
mod startup_tests;

fn plain() -> ContainerRequest<Nats> {
    Nats::default().with_tag("2.10.14").with_cmd(["--jetstream"])
}

#[test]
fn configured_or_authenticated_requests_never_enable_collection() {
    for request in [
        plain().with_cmd(["--auth", "sensitive-fixture-marker"]),
        plain().with_cmd(["--config", "/fixture.conf"]),
        plain().with_cmd(["--tls", "--tlskey", "/fixture.key"]),
        plain().with_env_var("FIXTURE_TOKEN", "sensitive-fixture-marker"),
        plain().with_copy_to("/fixture.conf", b"sensitive-fixture-marker".to_vec()),
        plain().with_mount(Mount::bind_mount("/fixture", "/fixture")),
        plain().with_name("unrelated-image"),
        plain().with_tag("unrelated-tag"),
        plain().with_container_name("unrelated-container"),
    ] {
        assert!(FixtureDiagnostics::for_request(&request).is_none());
    }
    assert!(FixtureDiagnostics::for_request(&plain()).is_some());
}

#[test]
fn split_warnings_survive_but_unknown_messages_and_payloads_do_not() {
    let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
    diagnostics.stderr(b"[1] 2026/11/09 00:00:00 [WRN] Stream create failed for '$G > AU");
    diagnostics.stderr(b"DIT': storage directory is not a directory (10077)\n");
    diagnostics.stderr(b"[1] 2026/11/09 00:00:00 [INF] sensitive-fixture-marker\n");
    diagnostics.stderr(b"[1] 2026/11/09 00:00:00 [ERR] sensitive-fixture-marker\n");
    diagnostics.stderr(
        b"[1] 2026/11/09 00:00:00 [WRN] Stream create failed for '$G > sensitive-fixture-marker': arbitrary payload\n",
    );
    diagnostics.stderr(b"[1] 2026/11/09 00:00:00 [WRN] Stream create failed for '$G > AUDIT': storage directory is not writable sensitive-fixture-marker\n");
    diagnostics.stderr(b"[1] 2026/11/09 00:00:00 [WRN] Stream create failed for '$G > AUDIT': could not create message storage directory - mkdir /tmp/nats/jetstream/sensitive-fixture-marker: no such file or directory\n");
    let report = diagnostics.report();
    assert!(report.contains("storage directory is not a directory"));
    assert!(report.contains("mkdir <fixture-store-path>: no such file or directory"));
    assert!(report.contains("unrecognized detail suppressed"));
    assert!(!report.contains("sensitive-fixture-marker"));
    assert!(!report.contains("arbitrary payload"));
    assert!(
        selected_log("[1] 2026/11/09 00:00:00 [WRN] arbitrary [INF] Server is ready")
            .is_some_and(|line| !line.contains("Server is ready"))
    );
    assert!(safe_error("sensitive-fixture-marker").is_none());
}

#[test]
fn oversized_lines_and_streams_are_bounded_without_poisoning_the_failure_tail() {
    let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
    diagnostics.stderr(&vec![b'x'; constants::MAX_LINE + 1]);
    diagnostics.stderr(b" [INF] Server is ready\n");
    assert!(!diagnostics.report().contains("Server is ready"));
    diagnostics.stderr(&vec![b'x'; constants::MAX_INPUT * 2]);
    let mut tail = StderrSelection::new();
    diagnostics.selected_stderr(
        &mut tail,
        b"[1] 2026/11/09 00:00:00 [WRN] Stream create failed for '$G > AUDIT': permission denied\n",
    );
    for _ in 0..constants::MAX_RECORDS * 2 {
        diagnostics.record(FixtureObservation::BrokerStderr {
            broker_stderr: "bounded",
        });
    }
    assert_eq!(
        diagnostics.evidence.lock().unwrap().records.len(),
        constants::MAX_RECORDS
    );
    assert!(diagnostics.report().len() < constants::MAX_RECORDS * 512);
    assert!(diagnostics.report().contains("diagnostics_truncated=true"));
    assert!(diagnostics.live_stderr.lock().unwrap().pending.len() <= constants::MAX_LINE);
    assert_eq!(diagnostics.live_stderr.lock().unwrap().remaining, 0);
    diagnostics.selected_stderr(
        &mut tail,
        b"[1] 2026/11/09 00:00:00 [WRN] Stream create failed for '$G > AUDIT': permission denied\n",
    );
    assert!(diagnostics.report().contains("AUDIT': permission denied"));
}

#[test]
fn inspect_selection_excludes_environment_paths_configuration_and_unknown_errors() {
    let inspected: ContainerInspectResponse = serde_json::from_value(json!({
        "Id": "a".repeat(64),
        "Image": format!("sha256:{}", "b".repeat(64)),
        "State": {"Status": "exited", "Running": false, "OOMKilled": true, "ExitCode": 42, "Error": "sensitive-fixture-marker"},
        "Config": {"Env": ["FIXTURE_TOKEN=sensitive-fixture-marker"], "Cmd": ["sensitive-fixture-marker"], "Labels": {"sensitive-fixture-marker": "sensitive-fixture-marker"}},
        "LogPath": "/sensitive-fixture-marker",
        "GraphDriver": {"Name": "overlay2", "Data": {"UpperDir": "/sensitive-fixture-marker"}},
        "Mounts": [{"Type": "bind", "Source": "/sensitive-fixture-marker", "Destination": "/sensitive-fixture-marker"}],
        "SizeRw": 123,
    })).unwrap();
    let selected = serde_json::to_value(selected_container_state(&inspected)).unwrap();
    assert_eq!(selected["state"], "exited");
    assert_eq!(selected["oom_killed"], true);
    assert_eq!(selected["exit_code"], 42);
    assert_eq!(selected["state_error_present"], true);
    assert_eq!(selected["state_error_selected"], Value::Null);
    assert_eq!(selected["size_rw"], 123);
    assert!(!selected.to_string().contains("sensitive-fixture-marker"));
}

#[test]
fn one_log_delivery_retains_only_the_bounded_tail_and_recovers_after_invalid_lines() {
    let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
    let warning = "[1] 2026/11/09 00:00:00 [WRN] permission denied\n";
    diagnostics.stderr(warning.repeat(constants::MAX_RECORDS + 1).as_bytes());
    diagnostics.stderr(b"\xff\n[caller] 2026/11/09 [INF] Server is ready\n[1] caller [INF] Server is ready\n[1] 2026/11/09 [DBG] sensitive-fixture-marker\n");
    diagnostics.stderr(b"[1] 2026/11/09 [INF] Server is ready\n");
    let report = diagnostics.report();
    assert!(report.contains("diagnostics_truncated=true"));
    assert_eq!(
        report.lines().filter(|line| line.starts_with('{')).count(),
        constants::MAX_RECORDS
    );
    assert_eq!(report.matches("Server is ready").count(), 1);
    assert!(!report.contains("caller"));
    assert!(!report.contains("sensitive-fixture-marker"));
}

#[test]
fn filesystem_warning_selection_preserves_causes_without_paths_or_unknown_context() {
    let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
    for cause in [
        "no space left on device",
        "permission denied",
        "no such file or directory",
        "not a directory",
        "file exists",
        "input/output error",
        "read-only file system",
        "too many open files",
        "too many open files in system",
    ] {
        diagnostics.stderr(format!("[1] 2026/11/09 [ERR] {cause}\n").as_bytes());
        assert!(diagnostics.report().contains(cause));
    }
    for detail in [
        "storage directory is not writable",
        "could not create storage directory - mkdir /tmp/nats/jetstream/sensitive-fixture-marker: permission denied",
        "could not create consumer storage directory - open /tmp/nats/jetstream/sensitive-fixture-marker: read-only file system",
        "write /tmp/nats/jetstream/sensitive-fixture-marker: no space left on device",
        "mkdir: permission denied",
        "unlink /tmp/nats/jetstream/sensitive-fixture-marker: permission denied",
        "open /sensitive-fixture-marker: permission denied",
        "open /tmp/nats/jetstream/sensitive-fixture-marker: unknown-cause",
    ] {
        diagnostics.stderr(format!("[1] 2026/11/09 [WRN] {detail}\n").as_bytes());
    }
    let report = diagnostics.report();
    assert!(report.contains("storage directory is not writable"));
    assert!(report.contains("mkdir <fixture-store-path>: permission denied"));
    assert!(report.contains("open <fixture-store-path>: read-only file system"));
    assert!(report.contains("write <fixture-store-path>: no space left on device"));
    assert_eq!(report.matches("unrecognized detail suppressed").count(), 4);
    assert!(!report.contains("sensitive-fixture-marker"));
    assert!(!report.contains("unknown-cause"));
    assert!(!report.contains("/tmp/"));
}
