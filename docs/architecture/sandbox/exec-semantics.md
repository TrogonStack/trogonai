# Exec semantics across workers

`run_command` runs on very different backends: a local container, a VM guest
agent, a pod exec, or a provider's HTTP API. This page explains which parts of
running a command behave the same on every worker and which parts stay
specific to the backend. The conformance suite in
`rsworkspace/crates/sandbox/trogon-sandbox-conformance` checks the shared parts.

## What every worker agrees on

- **Relative cwd.** A relative `cwd` joins the directory the command would run
  in with no `cwd`. So `cwd: "src"` on a lease whose default is `/workspace`
  runs in `/workspace/src`, whichever worker holds the lease. Workers use
  `resolve_exec_cwd` from `trogon-sandbox-protocol`, or leave a relative path to a
  shell that already starts in the default directory.
- **Missing cwd.** A `cwd` that does not exist never runs the command. The
  worker either denies the request or completes it with a nonzero exit, the
  way a shell's failed `cd` does (exit 1, `cd: <dir>: No such file or
  directory` on stderr where the worker controls the message).
- **Timeout.** A command stopped by its deadline completes with
  `timed_out: true` and `exit_code` set to `CommandOutput::TIMED_OUT_EXIT_CODE`
  (-1) when the worker can confirm that cause. A provider's terminal exit code
  is preserved when its cause is unavailable.

## Deadlines and uncertain results

A detached command keeps its finite command deadline. Providers that support
`deadline: "lease"` allow a detached command to run until explicit cleanup or
the lease ends. This explicit choice is useful for a development server that
must outlive an individual client request.

A request deadline can expire while the command continues running. Failed
signaling or a missing native exit receipt therefore produces a backend
error. Detached polling reports the uncertainty while the worker retains the
original execution and cleanup control. Keep that execution handle for kill
or stop retries. Lease disposal also retains failed cleanup work until the
provider confirms it has finished.

Process termination and output collection have separate evidence. A known
native exit can arrive before stdout or stderr finishes transferring. For a
normal completion, failed collection remains an explicit error until the
original output can be read; successful empty output requires a successful
read. A deadline result can contain only the output captured before cleanup.
Native exit codes such
as 124 and 137 retain their value unless the worker has evidence that its
deadline caused termination.

When a command has a `launch_key`, the broker retains delivered failures that
may follow execution, including backend and oversized-output errors. A retry
with the same key replays that result. A new key selects a new launch. Proven
validation refusals and definite nondelivery allow a same-key retry to
dispatch again.

## Why the default cwd is not normalized

Each worker keeps its own default directory. Forcing one path, such as
`/workspace`, on every worker would fight the backends:

- Images and provider sandboxes already pick a directory that the default user
  can write. Overriding it can land a command in a directory that does not
  exist or that the user cannot write.
- Providers document their own default (for example `/vercel/sandbox`), and
  callers who know a provider expect it.
- A caller who needs a specific directory can pass an absolute `cwd`, which
  every worker honors as is.

What the shared rule guarantees is that the same request resolves the same
way relative to whatever the default is.

## Default cwd by worker

| Worker | Default with no `cwd` |
|---|---|
| memory | `/` |
| docker | the container's working directory |
| fly, proxmox | the lease `working_dir`, else `/workspace` |
| firecracker, substrate (actor agent) | the lease `working_dir` when one is bound, else the agent's own directory. With `run_as_user`, the user's home |
| k8s | the container's working directory. With `run_as_user`, `/home/<user>` |
| daytona | `/workspace`. With a non-root `run_as_user`, `/home/<user>` |
| e2b | the user's home (`/home/<user>`, `/root` for root) |
| vercel | `/vercel/sandbox` |

## Workers without `run_command`

The subprocess worker serves no `run_command` verb: the lease's own process is its only output, so these rules do not apply to it.

The V8 worker accepts JavaScript through `["eval", source]` rather than an operating-system command.
