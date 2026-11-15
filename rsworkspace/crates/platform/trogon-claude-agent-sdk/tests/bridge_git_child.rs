//! The upstream bridge wraps its Git child in a sandbox, a memory cgroup and a launcher probe only when the host
//! switches them on, which `createCodeSession` options, the environment and server replies cannot do (the
//! `b01-bridge-git-child-wrappers` behavior row records the offsets). The Rust runner therefore starts the configured
//! program itself, with the fixed flags and the sanitised environment, and nothing in between.

#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use trogon_claude_agent_sdk::{DefaultGitProbe, GitContextProbe, GitProbeContext};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FLAGS: [&str; 4] = ["-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor="];

fn pair(key: &str, value: &str) -> (OsString, OsString) {
    (key.into(), value.into())
}

#[tokio::test]
async fn runs_plain_git_with_the_fixed_flags_and_the_sanitised_environment() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    let work = root.join("work");
    fs::create_dir_all(work.join(".git"))?;
    fs::write(work.join(".git/HEAD"), "ref: refs/heads/.invalid\n")?;
    let log = root.join("calls.log");
    let program = root.join("fake-git");
    fs::write(
        &program,
        format!(
            "#!/bin/sh\n{{ printf 'program=%s\\n' \"$0\"; printf 'arg=%s\\n' \"$@\"; env | sort; echo end; }} >> '{}'\n\
             case \"$5\" in\n  symbolic-ref) printf 'refs/remotes/origin/main\\n' ;;\n  show-ref) printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\n' ;;\nesac\n",
            log.display()
        ),
    )?;
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755))?;

    let mut context = GitProbeContext::from_process().with_session_cwd(&work);
    context.git_program = program.clone();
    context.environment = vec![
        pair("PATH", "/usr/bin:/bin"),
        pair("GIT_CONFIG_GLOBAL", "/elsewhere"),
        pair("GIT_SSH_COMMAND", "ssh -v"),
        pair("KEPT", "1"),
    ];
    let probe = DefaultGitProbe::with_directory(&work).with_context(context);
    assert_eq!(probe.guess_default_branch().await.as_deref(), Some("main"));

    let logged = fs::read_to_string(&log)?;
    let calls: Vec<&str> = logged.split("end\n").filter(|call| !call.is_empty()).collect();
    assert_eq!(calls.len(), 2);
    let git_dir = work.join(".git");
    for call in calls {
        let lines: Vec<&str> = call.lines().collect();
        assert_eq!(lines[0], format!("program={}", program.display()));
        let args: Vec<&str> = lines[1..].iter().map_while(|line| line.strip_prefix("arg=")).collect();
        assert_eq!(args[..FLAGS.len()], FLAGS);
        let environment = &lines[1 + args.len()..];
        for expected in [
            "GIT_ALLOW_PROTOCOL=/",
            "GIT_NO_LAZY_FETCH=1",
            "GIT_NO_REPLACE_OBJECTS=1",
            "GIT_TERMINAL_PROMPT=0",
            "GIT_LFS_SKIP_SMUDGE=1",
            "LC_ALL=C",
            "LANGUAGE=C",
            "GIT_SSH_COMMAND=false",
            "GIT_IMPLICIT_WORK_TREE=0",
            "KEPT=1",
            "PATH=/usr/bin:/bin",
        ] {
            assert!(environment.contains(&expected), "{expected}");
        }
        assert!(environment.contains(&format!("GIT_DIR={}", git_dir.display()).as_str()));
        assert!(environment.contains(&format!("GIT_COMMON_DIR={}", git_dir.display()).as_str()));
        assert!(
            environment
                .iter()
                .any(|line| line.starts_with("GIT_CONFIG_PARAMETERS='alias.remote-none=version'"))
        );
        assert!(!environment.iter().any(|line| line.starts_with("GIT_CONFIG_GLOBAL=")));
    }
    Ok(())
}

#[test]
fn keeps_the_program_a_plain_path() {
    let context = GitProbeContext::from_process();
    assert_eq!(context.git_program, Path::new("git"));
}
