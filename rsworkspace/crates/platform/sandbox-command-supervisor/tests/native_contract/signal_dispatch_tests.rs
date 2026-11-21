use super::*;

#[test]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn one_signal_reaches_adopted_nested_and_escaped_processes_once() {
    let _custody = TestCustody::enter();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    std::fs::write(
        path.join("leaf.sh"),
        r#"directory=$1
role=$2
trap 'printf X >> "$directory/$role-signals"' USR1
printf '%s\n' "$$" > "$directory/$role-pid"
while [ ! -f "$directory/finish" ]; do :; done
exit 7
"#,
    )
    .unwrap();
    std::fs::write(
        path.join("adopt.sh"),
        r#"/bin/sh "$1/leaf.sh" "$1" adopted &
exit 0
"#,
    )
    .unwrap();
    std::fs::write(
        path.join("middle.sh"),
        r#"directory=$1
trap 'printf X >> "$directory/middle-signals"' USR1
/bin/sh "$directory/leaf.sh" "$directory" nested &
/usr/bin/setsid /bin/sh "$directory/leaf.sh" "$directory" escaped &
printf '%s\n' "$$" > "$directory/middle-pid"
while [ ! -f "$directory/finish" ]; do :; done
wait
exit 7
"#,
    )
    .unwrap();
    std::fs::write(
        path.join("root.sh"),
        r#"directory=$1
trap 'printf X >> "$directory/root-signals"' USR1
/bin/sh "$directory/adopt.sh" "$directory" &
wait $!
/bin/sh "$directory/middle.sh" "$directory" &
printf '%s\n' "$$" > "$directory/root-pid"
while [ ! -f "$directory/finish" ]; do :; done
wait
exit 7
"#,
    )
    .unwrap();
    let mut sentinel = Sentinel::start();
    let mut command = Process::new(env!("CARGO_BIN_EXE_sandbox-command-supervisor"));
    let pending = syscall_notifications::PendingListener::install(
        &mut command,
        syscall_notifications::Syscall::SignalSend,
        syscall_notifications::Policy::observe(),
    )
    .unwrap();
    let mut helper = Harness::start_command(command);
    let listener = pending.start().unwrap();
    helper.send(Command::Init {
        version: VERSION,
        execution_id: helper.execution.clone(),
        argv: vec![
            "/bin/sh".into(),
            path.join("root.sh").to_string_lossy().into_owned(),
            path.to_string_lossy().into_owned(),
        ],
        env: workload_environment(BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())])),
        cwd: None,
        timeout_ms: None,
        release_timeout_ms: None,
    });
    let root = match helper.next() {
        Event::Ready { root, .. } => root,
        _ => panic!("signal tree requires a held root"),
    };
    helper.release();
    let until = Instant::now() + WAIT;
    let roles = ["root", "adopted", "middle", "nested", "escaped"];
    let mut identities = BTreeMap::new();
    for role in roles {
        let marker = path.join(format!("{role}-pid"));
        let pid = loop {
            if let Ok(text) = std::fs::read_to_string(&marker)
                && let Some(pid) = text.strip_suffix('\n').and_then(|text| text.parse::<u32>().ok())
            {
                break pid;
            }
            assert!(Instant::now() < until, "each signal target must become ready");
            std::thread::sleep(Duration::from_millis(5));
        };
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let fields: Vec<_> = stat[stat.rfind(')').unwrap() + 1..].split_whitespace().collect();
        identities.insert(
            role,
            (
                pin(Identity {
                    pid,
                    start_time: fields[19].parse().unwrap(),
                }),
                fields[1].parse::<u32>().unwrap(),
                fields[2].parse::<u32>().unwrap(),
            ),
        );
    }
    assert_eq!(identities["adopted"].1, helper.process.id());
    assert_eq!(identities["adopted"].2, root.pid);
    assert_eq!(identities["nested"].2, root.pid);
    assert_ne!(identities["escaped"].2, root.pid);
    assert_ne!(identities["escaped"].1, helper.process.id());
    sentinel.assert_alive();
    helper.signal(libc::SIGUSR1);
    let until = Instant::now() + WAIT;
    while roles
        .iter()
        .any(|role| !std::fs::read(path.join(format!("{role}-signals"))).is_ok_and(|bytes| bytes == b"X"))
        && Instant::now() < until
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    let effects = roles.map(|role| {
        (
            role,
            std::fs::read(path.join(format!("{role}-signals"))).unwrap_or_default(),
        )
    });
    std::fs::write(path.join("finish"), b"finish").unwrap();
    let absence = helper.absent();
    for (descriptor, _, _) in identities.values() {
        assert_dead(descriptor);
    }
    helper.acknowledge();
    let report = listener.finish().unwrap();
    sentinel.assert_alive();
    assert!(matches!(
        absence,
        Event::FamilyAbsent {
            root_exit_code: Some(7),
            root_signal: None,
            released: ReleaseState::Released,
            timed_out: TimeoutState::WithinDeadline,
            canceled: Cancellation::Active,
            ..
        }
    ));
    assert_eq!(
        report.signal_sends,
        BTreeMap::new(),
        "group sends overlap adopted pidfd targets"
    );
    assert_eq!(report.pidfd_sends, BTreeMap::from([(libc::SIGUSR1, 5)]));
    assert_eq!(effects, roles.map(|role| (role, b"X".to_vec())));
    assert_eq!(report.continued, 5);
    assert_eq!(report.refused, 0);
}

#[test]
#[expect(
    clippy::unwrap_used,
    reason = "A failed native fixture or contract assertion must terminate the test immediately"
)]
fn signals_include_children_created_by_nonleader_threads() {
    let _custody = TestCustody::enter();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    let mut sentinel = Sentinel::start();
    let mut command = Process::new(env!("CARGO_BIN_EXE_sandbox-command-supervisor"));
    let pending = syscall_notifications::PendingListener::install(
        &mut command,
        syscall_notifications::Syscall::SignalSend,
        syscall_notifications::Policy::observe(),
    )
    .unwrap();
    let mut helper = Harness::start_command(command);
    let listener = pending.start().unwrap();
    let root_identity = helper.initialize("signal-threads", path, None, None);
    let root = pin(root_identity);
    helper.release();
    let until = Instant::now() + WAIT;
    let child_pid = loop {
        if let Ok(text) = std::fs::read_to_string(path.join("child-pid"))
            && let Some(pid) = text.strip_suffix('\n').and_then(|text| text.parse::<u32>().ok())
        {
            break pid;
        }
        assert!(Instant::now() < until, "thread-owned child must become ready");
        std::thread::sleep(Duration::from_millis(5));
    };
    let stat = std::fs::read_to_string(format!("/proc/{child_pid}/stat")).unwrap();
    let fields: Vec<_> = stat[stat.rfind(')').unwrap() + 1..].split_whitespace().collect();
    assert_eq!(fields[1].parse::<u32>().unwrap(), root_identity.pid);
    let child = pin(Identity {
        pid: child_pid,
        start_time: fields[19].parse().unwrap(),
    });
    let leader_children = std::fs::read_to_string(format!("/proc/{0}/task/{0}/children", root_identity.pid,)).unwrap();
    assert!(
        !leader_children
            .split_whitespace()
            .any(|text| text == child_pid.to_string())
    );
    sentinel.assert_alive();
    helper.signal(libc::SIGUSR1);
    let until = Instant::now() + WAIT;
    while ["root", "child"]
        .iter()
        .any(|role| !std::fs::read(path.join(format!("{role}-signals"))).is_ok_and(|bytes| bytes == b"X"))
        && Instant::now() < until
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    let root_effect = std::fs::read(path.join("root-signals")).unwrap_or_default();
    let child_effect = std::fs::read(path.join("child-signals")).unwrap_or_default();
    std::fs::write(path.join("finish"), b"finish").unwrap();
    let absence = helper.absent();
    assert_dead(&root);
    assert_dead(&child);
    helper.acknowledge();
    let report = listener.finish().unwrap();
    sentinel.assert_alive();
    assert!(matches!(
        absence,
        Event::FamilyAbsent {
            root_exit_code: Some(7),
            root_signal: None,
            released: ReleaseState::Released,
            timed_out: TimeoutState::WithinDeadline,
            canceled: Cancellation::Active,
            ..
        }
    ));
    assert_eq!(root_effect, b"X");
    assert_eq!(child_effect, b"X");
    assert_eq!(report.signal_sends, BTreeMap::new());
    assert_eq!(report.pidfd_sends, BTreeMap::from([(libc::SIGUSR1, 2)]));
    assert_eq!(report.continued, 2);
    assert_eq!(report.refused, 0);
}
