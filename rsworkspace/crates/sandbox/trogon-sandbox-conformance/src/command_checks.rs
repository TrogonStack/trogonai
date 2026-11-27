use trogon_sandbox_contract::{
    CommandOutput, KillCommandResponse, PollCommandResponse, ReadFileResponse, RunCommandResponse, StopCommandResponse,
    WriteFilesResponse,
};

use crate::check_failure::ensure;
use crate::encoding::text;
use crate::held::Held;
use crate::run_fields::RunFields;
use crate::{CheckFuture, outcomes};

pub(crate) fn run_returns_stdout_and_exit_zero<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let out = outcomes::completed(l.run(&["echo", "hello"], false).await?)?;
        ensure(
            out.exit_code == 0 && text(&out.stdout).contains("hello"),
            "exit 0 and hello on stdout",
            &out,
        )
    })
}

pub(crate) fn run_propagates_nonzero_exit<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let out = outcomes::completed(l.run(&["false"], false).await?)?;
        ensure(out.exit_code != 0, "a nonzero exit", &out)
    })
}

pub(crate) fn run_unknown_command_fails<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let strict = l.target.strict_unknown_command_exit;
        let reply = l.run(&["definitely-not-a-command-xyz"], false).await?;
        let exact = matches!(&reply, RunCommandResponse::Completed(out) if strict && out.exit_code == 127);
        let failed = matches!(&reply, RunCommandResponse::Completed(out) if !strict && out.exit_code != 0);
        let denied = matches!(&reply, RunCommandResponse::Denied(_) if !strict);
        ensure(exact | failed | denied, format!("a failure (strict={strict})"), &reply)
    })
}

pub(crate) fn run_reports_the_exact_exit_code<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let out = outcomes::completed(l.run(&["sh", "-c", "exit 42"], false).await?)?;
        let ok = out.exit_code == 42 && !out.timed_out.is_timed_out();
        ensure(ok, "exit 42 as a result", &out)
    })
}

pub(crate) fn cwd_is_honored_and_a_relative_cwd_resolves_against_the_default<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let absolute = "/tmp/conformance-cwd";
        l.write(&format!("{absolute}/marker"), b"x").await?;
        let pwd = l.stdout_of(&["pwd"], RunFields::cwd(absolute)).await?;
        ensure(pwd == absolute, format!("pwd in {absolute}"), &pwd)?;
        let base = l.stdout_of(&["pwd"], RunFields::default()).await?;
        let expected = format!("{}/conformance-rel", base.trim_end_matches('/'));
        l.write(&format!("{expected}/marker"), b"x").await?;
        let pwd = l.stdout_of(&["pwd"], RunFields::cwd("conformance-rel")).await?;
        let message = format!("a relative cwd to resolve against {base:?} to {expected:?}");
        ensure(pwd == expected, message, &pwd)
    })
}

pub(crate) fn missing_cwd_does_not_run_the_command<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let reply = l
            .run_with(&["echo", "ran"], RunFields::cwd("/conformance-no-such-dir"))
            .await?;
        let refused = matches!(&reply, RunCommandResponse::Denied(_));
        let failed = matches!(&reply, RunCommandResponse::Completed(out) if out.exit_code != 0 && !ran(out));
        ensure(refused | failed, "a missing cwd to fail up front", &reply)
    })
}

fn ran(out: &CommandOutput) -> bool {
    text(&out.stdout).contains("ran")
}

pub(crate) fn per_call_env_is_visible<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let value = l
            .stdout_of(
                &["printenv", "CONFORMANCE_VAR"],
                RunFields::env("CONFORMANCE_VAR", "per call value"),
            )
            .await?;
        ensure(value == "per call value", "printenv to print the value", &value)
    })
}

pub(crate) fn closed_stdin_reads_end_of_file<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        // `cat` with no file reads stdin, so it only finishes if stdin is closed.
        let out = l.stdout_of(&["cat"], RunFields::timeout(10)).await?;
        ensure(out.is_empty(), "cat of a closed stdin to print nothing", &out)
    })
}

pub(crate) fn timeout_is_reported_with_the_shared_exit_code<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let reply = l.run_with(&["sleep", "30"], RunFields::timeout(1)).await?;
        ensure_timed_out(&reply)
    })
}

/// Passes when the reply is a command cut off by its deadline with the shared exit code.
pub(crate) fn ensure_timed_out(reply: &RunCommandResponse) -> Result<(), crate::CheckFailureError> {
    let expected = CommandOutput::TIMED_OUT_EXIT_CODE;
    let ok = matches!(reply, RunCommandResponse::Completed(out) if out.timed_out.is_timed_out() && out.exit_code == expected);
    ensure(ok, format!("timed_out with exit {expected}"), reply)
}

pub(crate) fn files_round_trip_and_commands_see_them<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let path = "/tmp/conformance-a.txt";
        let content = b"line one\n\xff\xfe binary tail".to_vec();
        let written = l.write(path, &content).await?;
        let ok = matches!(written, WriteFilesResponse::Written);
        ensure(ok, "the file to be written", &written)?;
        let read = l.read(path, l.target.timeout).await?;
        let ok = matches!(&read, ReadFileResponse::Content { data } if *data == content);
        ensure(ok, "the written bytes", &read)?;
        let out = outcomes::completed(l.run(&["cat", path], false).await?)?;
        let ok = out.exit_code == 0 && out.stdout == content;
        ensure(ok, "cat to see the written file", &out)
    })
}

pub(crate) fn detached_command_is_pollable<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let id = outcomes::started(l.run(&["echo", "polled"], true).await?)?;
        let out = l.wait(&id).await?;
        ensure(
            out.exit_code == 0 && text(&out.stdout).contains("polled"),
            "exit 0 and polled on stdout",
            &out,
        )
    })
}

pub(crate) fn kill_stops_a_running_command<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let id = outcomes::started(l.run(&["sleep", "300"], true).await?)?;
        let polled = l.poll(&id, l.target.timeout).await?;
        let ok = matches!(polled, PollCommandResponse::Running);
        ensure(ok, "the command running", &polled)?;
        let killed = l.kill(&id).await?;
        let ok = matches!(killed, KillCommandResponse::Signaled);
        ensure(ok, "signaled", &killed)?;
        let out = l.wait(&id).await?;
        ensure(out.exit_code != 0, "a killed command not to exit zero", &out)?;
        let again = l.kill(&id).await?;
        let ok = matches!(
            again,
            KillCommandResponse::AlreadyCompleted | KillCommandResponse::NotFound
        );
        ensure(ok, "nothing to kill in a finished command", &again)
    })
}

pub(crate) fn missing_things_are_not_found<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let missing = crate::ids::fresh_execution_id();
        let read = l.read("/tmp/conformance-does-not-exist", l.target.timeout).await?;
        let ok = matches!(read, ReadFileResponse::NotFound | ReadFileResponse::Denied(_));
        ensure(ok, "a missing file to be not found", &read)?;
        let polled = l.poll(&missing, l.target.timeout).await?;
        let ok = matches!(polled, PollCommandResponse::NotFound | PollCommandResponse::Denied(_));
        ensure(ok, "an unknown execution to be not found", &polled)?;
        let killed = l.kill(&missing).await?;
        let ok = matches!(killed, KillCommandResponse::NotFound | KillCommandResponse::Denied(_));
        ensure(ok, "an unknown execution to be not found", &killed)
    })
}

/// Stopping a running detached command ends it, and stopping it again is not an error.
pub(crate) fn graceful_stop_ends_a_detached_command<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let id = outcomes::started(l.run(&["sleep", "300"], true).await?)?;
        let stopped = l.stop(&id).await?;
        let ok = matches!(stopped, StopCommandResponse::Stopped { .. });
        ensure(ok, "stopped", &stopped)?;
        let polled = l.poll(&id, l.target.timeout).await?;
        let ok = matches!(polled, PollCommandResponse::Completed(_));
        ensure(ok, "completed after stop", &polled)?;
        let again = l.stop(&id).await?;
        let ok = matches!(
            again,
            StopCommandResponse::AlreadyCompleted | StopCommandResponse::NotFound
        );
        ensure(ok, "nothing to stop in a finished command", &again)?;
        let unknown = l.stop(&crate::ids::fresh_execution_id()).await?;
        let ok = matches!(unknown, StopCommandResponse::NotFound | StopCommandResponse::Denied(_));
        ensure(ok, "an unknown execution to be not found", &unknown)
    })
}

/// Restart ends the old execution and starts the same command under a new execution id.
pub(crate) fn restart_runs_the_command_again_under_a_new_id<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let old = outcomes::started(l.run(&["sleep", "300"], true).await?)?;
        let new = outcomes::restarted(l.restart(&old).await?)?;
        ensure(new != old, "a new execution id", &new)?;
        let polled = l.poll(&new, l.target.timeout).await?;
        let ok = matches!(polled, PollCommandResponse::Running);
        ensure(ok, "the restarted command running", &polled)?;
        let killed = l.kill(&new).await?;
        let ok = matches!(killed, KillCommandResponse::Signaled);
        ensure(ok, "the restarted command signaled", &killed)
    })
}
