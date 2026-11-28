use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use clap::Parser;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::CapabilitySupport::{Supported, Unsupported};
use trogon_sandbox_contract::{
    CommandOutput, EgressCapability, EgressEnforcementLevel, FileContentHashing, FileRecursion, ProviderKind,
    SuspendLevel, WorkerId,
};
use trogon_sandbox_memory_worker::{
    CommandCtx, CommandTable, LiveOutput, MemoryFs, MemoryWorkerArgs, PROFILE, build_heartbeat,
    build_heartbeat_in_regions, out, resolve,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn ctx(fs: &Arc<Mutex<MemoryFs>>, env: &[(&str, &str)]) -> CommandCtx {
    CommandCtx {
        args: Vec::new(),
        cwd: "/".to_string(),
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
        fs: Arc::clone(fs),
        cancel: CancellationToken::new(),
        live: LiveOutput::default(),
    }
}

async fn run(table: &CommandTable, command: &[&str], context: CommandCtx) -> CommandOutput {
    let command: Vec<String> = command.iter().map(|s| s.to_string()).collect();
    table.run(&command, context).await
}

#[test]
fn resolve_collapses_relative_segments() {
    assert_eq!(resolve("/a/b", "../c/./d"), "/a/c/d");
    assert_eq!(resolve("/a", "/x//y"), "/x/y");
    assert_eq!(resolve("/", ".."), "/");
}

#[test]
fn the_file_map_resolves_paths_and_directories() -> TestResult {
    let mut fs = MemoryFs::default();
    assert!(fs.is_empty());
    fs.write("/w/a/f", b"1".to_vec());
    fs.write("/w/g", b"22".to_vec());
    assert_eq!(fs.len(), 2);
    assert_eq!(fs.read("w/g"), Some(&b"22"[..]));
    assert!(fs.exists("/w/a/f") && !fs.exists("/w/a"));
    assert!(fs.is_dir("/") && fs.is_dir("/tmp") && fs.is_dir("/w/a") && !fs.is_dir("/w/a/f") && !fs.is_dir("/zzz"));

    let shallow = fs.manifest("/w", FileRecursion::Flat, FileContentHashing::Metadata)?;
    assert_eq!(shallow.len(), 1);
    let deep = fs.manifest("/w", FileRecursion::Recursive, FileContentHashing::Content)?;
    assert_eq!(deep.len(), 2);
    let root = fs.manifest("/", FileRecursion::Recursive, FileContentHashing::Metadata)?;
    assert_eq!(root.len(), 2);
    let single = fs.manifest("/w/g", FileRecursion::Flat, FileContentHashing::Metadata)?;
    assert_eq!(single.len(), 1);
    Ok(())
}

#[tokio::test]
async fn the_default_commands_behave_like_their_namesakes() -> TestResult {
    let table = CommandTable::default();
    let fs = Arc::new(Mutex::new(MemoryFs::default()));
    fs.lock().map_err(|e| e.to_string())?.write("/tmp/a", b"A".to_vec());

    assert_eq!(run(&table, &["echo", "x", "y"], ctx(&fs, &[])).await.stdout, b"x y\n");
    assert_eq!(run(&table, &["true"], ctx(&fs, &[])).await.exit_code, 0);
    assert_eq!(run(&table, &["false"], ctx(&fs, &[])).await.exit_code, 1);
    assert_eq!(run(&table, &["pwd"], ctx(&fs, &[])).await.stdout, b"/\n");
    assert_eq!(
        run(&table, &["printenv"], ctx(&fs, &[("A", "1"), ("B", "2")]))
            .await
            .stdout,
        b"A=1\nB=2\n"
    );
    let found = run(&table, &["printenv", "A"], ctx(&fs, &[("A", "1")])).await;
    assert_eq!((found.exit_code, found.stdout), (0, b"1\n".to_vec()));
    assert_eq!(
        run(&table, &["printenv", "A", "Z"], ctx(&fs, &[("A", "1")]))
            .await
            .exit_code,
        1
    );
    assert_eq!(
        run(&table, &["cat", "/tmp/a", "/tmp/a"], ctx(&fs, &[])).await.stdout,
        b"AA"
    );
    let missing = run(&table, &["cat", "/tmp/a", "/tmp/none"], ctx(&fs, &[])).await;
    assert_eq!((missing.exit_code, missing.stdout), (1, b"A".to_vec()));
    assert!(missing.stderr.starts_with(b"cat: /tmp/none"));

    assert_eq!(run(&table, &["nope"], ctx(&fs, &[])).await.exit_code, 127);
    assert_eq!(run(&table, &[], ctx(&fs, &[])).await.stderr, b"empty command\n");
    assert_eq!(run(&table, &["sh", "-c", "exit"], ctx(&fs, &[])).await.exit_code, 0);
    assert_eq!(run(&table, &["sh", "-c", "exit 7"], ctx(&fs, &[])).await.exit_code, 7);
    assert_eq!(run(&table, &["sh", "-c", "exit x"], ctx(&fs, &[])).await.exit_code, 2);
    assert_eq!(
        run(&table, &["sh", "-c", "echo hi"], ctx(&fs, &[])).await.stdout,
        b"hi\n"
    );

    assert_eq!(run(&table, &["sleep", "0"], ctx(&fs, &[])).await.exit_code, 0);
    for argument in ["NaN", "abc", "-5", ""] {
        let refused = run(&table, &["sleep", argument], ctx(&fs, &[])).await;
        assert_eq!(refused.exit_code, 1);
        assert!(refused.stderr.starts_with(b"sleep: invalid time interval"));
    }
    assert_eq!(run(&table, &["sleep"], ctx(&fs, &[])).await.exit_code, 0);
    let cancelled = ctx(&fs, &[]);
    cancelled.cancel.cancel();
    assert_eq!(run(&table, &["sleep", "1e300"], cancelled).await.exit_code, 143);
    Ok(())
}

#[tokio::test]
async fn registered_commands_replace_the_defaults() {
    let mut table = CommandTable::empty();
    table.register_sync("echo", |_| out(5, "mine", "err"));
    table.register_sync("sh", |_| out(9, "", ""));
    let fs = Arc::new(Mutex::new(MemoryFs::default()));
    assert_eq!(run(&table, &["echo"], ctx(&fs, &[])).await.exit_code, 5);
    assert_eq!(run(&table, &["sh", "-c", "exit 1"], ctx(&fs, &[])).await.exit_code, 9);
    assert_eq!(run(&table, &["true"], ctx(&fs, &[])).await.exit_code, 127);
}

#[tokio::test]
async fn serve_announces_its_port_until_cancelled() {
    let table = CommandTable::default();
    let fs = Arc::new(Mutex::new(MemoryFs::default()));
    for (arguments, listening) in [(vec!["serve", "8080"], true), (vec!["serve"], false)] {
        let context = ctx(&fs, &[]);
        let live = context.live.clone();
        let cancel = context.cancel.clone();
        let task = tokio::spawn({
            let table = table.clone();
            async move { run(&table, &arguments, context).await }
        });
        while !live.contains(b"listening on") {
            tokio::task::yield_now().await;
        }
        assert_eq!(live.is_listening(8080), listening);
        cancel.cancel();
        assert!(matches!(task.await, Ok(output) if output.exit_code == 143));
    }
}

#[tokio::test]
async fn serve_refuses_a_port_that_is_not_a_port() {
    let table = CommandTable::default();
    let fs = Arc::new(Mutex::new(MemoryFs::default()));
    for argument in ["abc", "70000", "-1", ""] {
        let context = ctx(&fs, &[]);
        let live = context.live.clone();
        let refused = run(&table, &["serve", argument], context).await;
        assert_eq!(refused.exit_code, 2);
        assert!(refused.stderr.starts_with(b"serve: invalid port"));
        assert!(!live.contains(b"listening on"));
    }
}

#[test]
fn live_output_never_matches_an_empty_needle() {
    let live = LiveOutput::default();
    live.emit("abc");
    assert!(live.contains(b"bc") && !live.contains(b"") && !live.contains(b"abcd"));
    assert!(!live.is_listening(1));
    live.listen(1);
    assert!(live.is_listening(1));
}

#[test]
fn heartbeats_claim_the_declared_profile() -> TestResult {
    let worker = WorkerId::new("w1")?;
    let plain = build_heartbeat(ProviderKind::Mock, &worker, 8, 3);
    assert_eq!(
        (plain.capacity.max_concurrent_leases, plain.capacity.current_leases),
        (8, 3)
    );
    assert!(plain.profile.placement.regions.is_empty());
    let placed = build_heartbeat_in_regions(ProviderKind::Mock, &worker, 8, 0, &["eu-1".to_string()]);
    assert_eq!(placed.profile.placement.regions, vec!["eu-1".to_string()]);
    assert_eq!(PROFILE.provider, ProviderKind::Mock);
    Ok(())
}

#[test]
fn manifests_follow_content_and_skip_missing_paths() -> TestResult {
    let mut fs = MemoryFs::default();
    fs.write("/w/a.txt", b"1".to_vec());
    fs.write("/w/sub/b.txt", b"22".to_vec());
    fs.write("/other/c.txt", b"3".to_vec());
    let before = fs.manifest("/w", FileRecursion::Recursive, FileContentHashing::Content)?;
    assert_eq!(
        before.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
        vec!["/w/a.txt", "/w/sub/b.txt"]
    );
    assert_eq!(before.get("/w/sub/b.txt").map(|entry| entry.size()), Some(2));
    assert!(
        fs.manifest("/nowhere", FileRecursion::Recursive, FileContentHashing::Content)?
            .is_empty()
    );
    fs.write("/w/a.txt", b"2".to_vec());
    let after = fs.manifest("/w", FileRecursion::Recursive, FileContentHashing::Content)?;
    let hash =
        |manifest: &trogon_sandbox_file_ops::Manifest| manifest.get("/w/a.txt").and_then(|entry| entry.hash().cloned());
    assert!(hash(&before).is_some());
    assert_ne!(hash(&after), hash(&before));
    Ok(())
}

#[test]
fn a_sibling_with_a_shared_prefix_is_not_a_directory() {
    let mut fs = MemoryFs::default();
    fs.write("/w/sub/b.txt", b"x".to_vec());
    fs.write("/wx", b"x".to_vec());
    assert!(fs.is_dir("/w"));
    assert!(fs.is_dir("/w/sub/"));
    assert!(!fs.is_dir("/wx"));
    assert!(!fs.is_dir("/w/sub/b.txt"));
    assert_eq!(fs.read("/w/./sub/b.txt"), Some(&b"x"[..]));
    assert!(fs.read("/w/sub/c.txt").is_none());
}

#[tokio::test]
async fn commands_resolve_paths_against_their_working_directory() -> TestResult {
    let table = CommandTable::default();
    let fs = Arc::new(Mutex::new(MemoryFs::default()));
    fs.lock().map_err(|e| e.to_string())?.write("/work/f", b"data".to_vec());
    let mut context = ctx(&fs, &[]);
    context.cwd = "/work".to_string();
    let found = run(&table, &["cat", "f"], context).await;
    assert_eq!((found.exit_code, found.stdout), (0, b"data".to_vec()));
    let missing = run(&table, &["cat", "f"], ctx(&fs, &[])).await;
    assert_eq!(missing.exit_code, 1);
    Ok(())
}

#[tokio::test]
async fn a_shell_line_splits_on_any_run_of_whitespace() {
    let table = CommandTable::default();
    let fs = Arc::new(Mutex::new(MemoryFs::default()));
    let spaced = run(&table, &["sh", "-c", "echo  a   b"], ctx(&fs, &[])).await;
    assert_eq!((spaced.exit_code, spaced.stdout), (0, b"a b\n".to_vec()));
    assert_eq!(run(&table, &["sh", "-c", "nope"], ctx(&fs, &[])).await.exit_code, 127);
}

#[tokio::test]
async fn a_registered_handler_sees_its_arguments() {
    let mut table = CommandTable::default();
    table.register_sync("hello", |context| out(0, &format!("hi {}", context.args.join(",")), ""));
    let fs = Arc::new(Mutex::new(MemoryFs::default()));
    let greeted = run(&table, &["hello", "bob", "amy"], ctx(&fs, &[])).await;
    assert_eq!(greeted.stdout, b"hi bob,amy");
    assert_eq!(run(&table, &["echo", "still"], ctx(&fs, &[])).await.stdout, b"still\n");
}

#[test]
fn pause_is_a_freeze_and_suspend_is_memory_level() {
    assert_eq!(PROFILE.lifecycle.dormancy.freeze, Supported);
    assert_eq!(PROFILE.lifecycle.dormancy.suspend, Some(SuspendLevel::Memory));
}

#[test]
fn egress_claims_nothing_because_there_is_no_network_to_restrict() {
    assert_eq!(PROFILE.isolation.egress, EgressCapability::NONE);
    assert_eq!(PROFILE.isolation.egress_enforcement, EgressEnforcementLevel::None);
}

#[test]
fn the_profile_claims_only_what_the_worker_does() {
    assert_eq!(PROFILE.snapshot.can_snapshot, Unsupported);
    assert!(PROFILE.volume.supported_modes.is_empty());
    assert_eq!(PROFILE.isolation.supports_sandbox_users, Unsupported);
    assert_eq!(PROFILE.isolation.supports_interactive_stdin, Unsupported);
    assert_eq!(PROFILE.isolation.supports_file_ops, Unsupported);
    assert_eq!(PROFILE.isolation.supports_named_processes, Supported);
    assert_eq!(PROFILE.isolation.supports_process_readiness, Supported);
    assert_eq!(PROFILE.isolation.supports_output_redaction, Supported);
    assert_eq!(PROFILE.isolation.supports_change_tracking, Supported);
    assert_eq!(PROFILE.isolation.supports_file_watch, Supported);
}

#[test]
fn the_command_line_has_working_defaults() -> TestResult {
    let args = MemoryWorkerArgs::try_parse_from(["trogon-sandbox-memory-worker"])?;
    assert_eq!(args.nats_url, "nats://127.0.0.1:4222");
    assert_eq!(args.max_concurrent_leases, 64);
    assert!(args.regions.is_empty());
    assert!(args.dispatch_fence_file.is_none());
    let regions = MemoryWorkerArgs::try_parse_from(["trogon-sandbox-memory-worker", "--regions", "eu-1,us-2"])?;
    assert_eq!(regions.regions, vec!["eu-1".to_string(), "us-2".to_string()]);
    Ok(())
}
