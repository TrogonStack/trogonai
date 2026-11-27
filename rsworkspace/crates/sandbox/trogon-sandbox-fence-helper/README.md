# Sandbox fence helper

`trogon-sandbox-fence-helper` is the elevated half of the Windows sandbox backend. It
provisions the sandbox account, installs the Windows Filtering Platform (WFP) filters
that fence its network traffic, grants and stamps file ACLs for it, and launches a
command as that account under a restricted token. The host side lives in a separate
crate and starts this helper by an explicit path; nothing here depends on it.

The crate builds only for Windows. On other targets the library is empty and the
binary prints a notice and exits with status 2, so workspace-wide checks still pass.

`run_from_args` is the whole command line. The `trogon-sandbox-fence-helper` binary is
a one-line shim over it, and a multicall binary can link the crate and route to it when
its first argument equals `DISPATCH_ARG1` (`--fence-helper`). Dispatch keys on the first
argument rather than `argv[0]` because Windows does not preserve a spoofed `argv[0]`
across `CreateProcessWithLogonW` or `ShellExecuteExW`.

## Subcommands

| Command | Purpose |
| --- | --- |
| `install`, `uninstall` | Create or remove the `trogon-sandbox` account and its user-keyed WFP filters (one UAC prompt). |
| `status` | One line of JSON describing the account and the filters. |
| `user ...` | Inspect the account, read its credential, trust the CA. |
| `wfp ...` | Inspect, probe, or remove the filters. |
| `acl ...` | Additive sandbox-user ACEs, with write grants kept between sessions. |
| `profile ...` | The opt-in read grant on the real user's profile. |
| `exec` | Spawn a target under the two-hop sandbox-user lockdown. |

`status` subcommands write JSON to stdout and exit 0. Mutating subcommands require
elevation and write progress to stderr. `exec` returns the child's exit code.

## Modules

- `sam`, `sid`, `util`, `reg`, `dpapi`: account, SID, registry, and DPAPI primitives.
- `user`, `install`, `cert_store`: account lifecycle, the install record, and the CA.
- `wfp`, `probe`: filter management and the behavioural probe behind `wfp verify`.
- `acl`, `path_id`, `persist`, `state_db`, `ambient`: ACE grants, file identity, kept
  write grants, and the cross-process reference counts that decide when an ACE is removed.
- `profile`, `profile_plan`: the profile read grant and its pure planning half.
- `logon`, `runner`, `launch`, `token`, `job`, `winsta`, `self_protect`: the two-hop launch
  and its lockdown stack.

## Errors

Every fallible operation returns `FenceHelperError`, which wraps a system, storage, or
message failure in the context of what was being attempted. Display prints the
outermost context; the alternate form (`{:#}`) prints the whole chain. Conditions callers
branch on (a busy profile, a vanished holder, a mapped-drive working directory, a disabled
secondary logon service, a WFP access denial) are variants with typed accessors.

## Features

`test-hooks` exposes seams the integration smoke scripts under `ci/` use. It is off by
default.

## Tests

The unit and integration tests are Windows-only. The scripts under `ci/` run the built
binary end to end on a Windows machine; they are not wired into a workflow.
