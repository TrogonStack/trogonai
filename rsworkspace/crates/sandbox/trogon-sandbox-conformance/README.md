# Sandbox conformance

`trogon-sandbox-conformance` checks that a provider worker answers the provider subjects the way
the sandbox contract says it must. The checks talk to the worker over NATS, not through the
broker, so one suite covers any worker that serves those subjects.

## Use

Point a `Target` at a NATS server a worker is connected to, declare what the worker supports, and
call `assert_conforms`. It returns `Ok(())`, or `NotConformantError` listing every `SuiteFailureError` by
check name. `run_suite` returns the same failures as a list, and `checks`, `checks_for`, and
`checks_for_target` name the checks that apply, so a caller can run them one at a time.

A check only runs when the target declares the capability it exercises: `supports_pause_resume`,
`graceful_stop`, `command_restart`, `named_processes`, `process_readiness`, `native_passthrough`,
`file_ops`, `file_search`, `file_archive`, and `denied_connect_fails`. `ExecModel` says what
`run_command` means on the worker: a shell, a JavaScript evaluator, or nothing.

## Contents

- `Target` holds the connection, the declarations, and the lease request every check starts from.
  `for_worker` directs the requests at the worker that sent a heartbeat, using the dispatch
  protocol it declares.
- Signing: set `Target::signer`, or call `signed_from_env(&env)`, and the suite signs every dispatch and
  per-lease request the way the broker does, for a worker that runs with verification required.
  `signer_from_env(&env)` reads the same key variables as the broker from any `trogon_std::env::ReadEnv`.
- Grants: `GrantLedger` records each grant the suite received with the `NativeCleanupAuthority`
  it carries, so a caller can clean up what a failed check left behind. `AssignmentFaultError` names
  why a grant or request could not be tied to the worker assignment the suite retained.
- Errors: `CheckFailureError` says why one check did not pass.

## What it does not do

It does not start a worker, build an image, or reach the public internet. The egress check uses a
literal address and a positive control, so a lease under `DenyAll` is compared with an unrestricted
one rather than trusted on its own.

## Tests

The tests run the suite against an in-process worker under `tests/support` that serves the
provider subjects over a real NATS server and runs commands and file operations on the host. They
start the server in a container through `trogon-nats`'s `test-support` feature, so they need
Docker.
