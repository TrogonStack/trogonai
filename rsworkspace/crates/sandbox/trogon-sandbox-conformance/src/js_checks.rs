use trogon_sandbox_contract::{RunCommandResponse, WriteFilesResponse};

use crate::check_failure::ensure;
use crate::command_checks::ensure_timed_out;
use crate::constants::EVAL;
use crate::encoding::text;
use crate::held::Held;
use crate::run_fields::RunFields;
use crate::{CheckFuture, outcomes};

pub(crate) fn eval_returns_stdout_and_exit_zero<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let out = l
            .stdout_of(&[EVAL, "console.log('hello')"], RunFields::default())
            .await?;
        ensure(out.contains("hello"), "hello on stdout", &out)
    })
}

pub(crate) fn a_thrown_error_exits_one_with_the_message_on_stderr<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let out = outcomes::completed(l.run(&[EVAL, "throw new Error('conformance-boom')"], false).await?)?;
        ensure(
            out.exit_code == 1 && !out.timed_out.is_timed_out() && text(&out.stderr).contains("conformance-boom"),
            "exit 1 with the message on stderr",
            &out,
        )
    })
}

pub(crate) fn eval_timeout_is_reported_with_the_shared_exit_code<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let reply = l.run_with(&[EVAL, "for (;;) {}"], RunFields::timeout(1)).await?;
        ensure_timed_out(&reply)
    })
}

/// A JS runtime has no process, working directory or filesystem verbs: each of those requests gets a typed refusal
/// instead of a silent success.
pub(crate) fn what_a_js_runtime_cannot_honor_is_refused_with_a_typed_error<'a>(l: &'a Held<'a>) -> CheckFuture<'a> {
    Box::pin(async move {
        let refused: [(&[&str], RunFields); 4] = [
            (&["echo", "ran"], RunFields::default()),
            (&[EVAL, "1"], RunFields::detached(true)),
            (&[EVAL, "1"], RunFields::cwd("/tmp")),
            (&[EVAL, "1"], RunFields::env("CONFORMANCE_VAR", "x")),
        ];
        for (argv, fields) in refused {
            let reply = l.run_with(argv, fields.clone()).await?;
            let refused = matches!(reply, RunCommandResponse::Denied(_));
            let expected = format!("{argv:?} with {fields} to be refused");
            ensure(refused, expected, &reply)?;
        }
        let written = l.write("/tmp/conformance-js.txt", b"x").await?;
        ensure(
            matches!(written, WriteFilesResponse::Denied(_)),
            "write_files to be refused",
            &written,
        )
    })
}
