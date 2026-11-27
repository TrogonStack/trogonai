use crate::command_checks::*;
use crate::constants::{ANY, JS_EVAL, SHELL, STATEFUL};
use crate::egress_checks::denied_connect_fails;
use crate::file_verb_checks::*;
use crate::held::Held;
use crate::js_checks::*;
use crate::lease_checks::*;
use crate::process_checks::*;
use crate::{CheckFn, ExecModel, NotConformantError, SuiteFailureError, Target};

/// A check that leases a sandbox and always releases it.
macro_rules! check {
    ($f:ident, $models:expr) => {
        (stringify!($f), $models, (|t| Box::pin($f(t))) as CheckFn)
    };
}

/// A check that runs against a lease the suite holds, gated on a declaration of the worker.
macro_rules! leased {
    ($f:ident, $models:expr, $gate:expr) => {
        (
            stringify!($f),
            $models,
            (|t| {
                Box::pin(async move {
                    if !$gate(t) {
                        return Ok(());
                    }
                    with_lease(t, $f).await
                })
            }) as CheckFn,
        )
    };
}

async fn with_lease(
    target: &Target,
    body: for<'a> fn(&'a Held<'a>) -> crate::CheckFuture<'a>,
) -> Result<(), crate::CheckFailureError> {
    let held = Held::acquire(target).await?;
    let result = body(&held).await;
    held.release().await;
    result
}

type TableEntry = (&'static str, &'static [ExecModel], CheckFn);

fn always(_: &Target) -> bool {
    true
}

fn table() -> Vec<TableEntry> {
    vec![
        check!(lease_is_granted, ANY),
        leased!(run_returns_stdout_and_exit_zero, SHELL, always),
        leased!(run_propagates_nonzero_exit, SHELL, always),
        leased!(run_unknown_command_fails, SHELL, always),
        leased!(run_reports_the_exact_exit_code, SHELL, always),
        leased!(
            cwd_is_honored_and_a_relative_cwd_resolves_against_the_default,
            SHELL,
            always
        ),
        leased!(missing_cwd_does_not_run_the_command, SHELL, always),
        leased!(per_call_env_is_visible, SHELL, always),
        leased!(closed_stdin_reads_end_of_file, SHELL, always),
        leased!(timeout_is_reported_with_the_shared_exit_code, SHELL, always),
        leased!(files_round_trip_and_commands_see_them, SHELL, always),
        leased!(detached_command_is_pollable, SHELL, always),
        leased!(kill_stops_a_running_command, SHELL, always),
        leased!(missing_things_are_not_found, STATEFUL, always),
        leased!(pause_keeps_state_and_resume_restores_it, SHELL, |t: &Target| t
            .supports_pause_resume),
        leased!(graceful_stop_ends_a_detached_command, SHELL, |t: &Target| t
            .graceful_stop),
        leased!(restart_runs_the_command_again_under_a_new_id, SHELL, |t: &Target| t
            .command_restart),
        leased!(unsupported_checkpoint_running_gets_a_typed_reply, ANY, |t: &Target| t
            .checkpoint_running_unsupported),
        check!(cancel_releases_the_lease, STATEFUL),
        leased!(redispatching_a_lease_keeps_its_state, STATEFUL, |t: &Target| t
            .redispatch_idempotent),
        check!(unknown_native_key_is_refused_with_a_typed_error, ANY),
        leased!(
            named_process_is_idempotent_listed_and_freed_on_exit,
            SHELL,
            |t: &Target| t.named_processes
        ),
        leased!(
            process_readiness_reports_ready_not_ready_and_failed,
            SHELL,
            |t: &Target| t.named_processes && t.process_readiness
        ),
        leased!(eval_returns_stdout_and_exit_zero, JS_EVAL, always),
        leased!(a_thrown_error_exits_one_with_the_message_on_stderr, JS_EVAL, always),
        leased!(eval_timeout_is_reported_with_the_shared_exit_code, JS_EVAL, always),
        leased!(
            what_a_js_runtime_cannot_honor_is_refused_with_a_typed_error,
            JS_EVAL,
            always
        ),
    ]
}

fn file_checks(ops: bool, search: bool, archive: bool) -> Vec<(&'static str, CheckFn)> {
    let all: [(&'static str, bool, CheckFn); 4] = [
        ("file_verbs_round_trip", ops, |t| {
            Box::pin(with_lease(t, file_verbs_round_trip))
        }),
        ("file_verbs_report_missing_paths", ops, |t| {
            Box::pin(with_lease(t, file_verbs_report_missing_paths))
        }),
        ("file_search_finds_names_and_content", ops && search, |t| {
            Box::pin(with_lease(t, file_search_finds_names_and_content))
        }),
        ("file_archive_round_trips_a_tree", ops && archive, |t| {
            Box::pin(with_lease(t, file_archive_round_trips_a_tree))
        }),
    ];
    all.into_iter()
        .filter(|(_, enabled, _)| *enabled)
        .map(|(name, _, check)| (name, check))
        .collect()
}

/// Every check, by name.
pub fn checks() -> Vec<(&'static str, CheckFn)> {
    table()
        .into_iter()
        .map(|(name, _, check)| (name, check))
        .chain(file_checks(true, true, true))
        .chain([("denied_connect_fails", denied_connect_fails as CheckFn)])
        .collect()
}

/// The checks that apply to a worker with the given [`ExecModel`].
pub fn checks_for(exec: ExecModel) -> Vec<(&'static str, CheckFn)> {
    table()
        .into_iter()
        .filter(|(_, models, _)| models.contains(&exec))
        .map(|(name, _, check)| (name, check))
        .collect()
}

/// The checks that apply to this target: those its [`ExecModel`] admits plus the file verb checks and the egress
/// check its declarations enable.
pub fn checks_for_target(target: &Target) -> Vec<(&'static str, CheckFn)> {
    let egress = (target.exec == ExecModel::Shell && target.denied_connect_fails)
        .then_some(("denied_connect_fails", denied_connect_fails as CheckFn));
    checks_for(target.exec)
        .into_iter()
        .chain(file_checks(target.file_ops, target.file_search, target.file_archive))
        .chain(egress)
        .collect()
}

/// Runs every check that applies to the target and returns the ones that failed.
pub async fn run_suite(target: &Target) -> Vec<SuiteFailureError> {
    let mut failures = Vec::new();
    for (name, check) in checks_for_target(target) {
        if let Err(failure) = check(target).await {
            failures.push(SuiteFailureError { name, failure });
        }
    }
    failures
}

/// `Ok` when every check passes, otherwise every failure.
pub async fn assert_conforms(target: &Target) -> Result<(), NotConformantError> {
    let failures = run_suite(target).await;
    failures.is_empty().then_some(()).ok_or(NotConformantError(failures))
}

#[cfg(test)]
mod tests;
