# Sandbox policy exec

`trogon-sandbox-policy-exec` runs commands under a filesystem and network policy
from inside another Rust program, with no message bus. It confines each command
with the same isolation backends the sandbox worker uses, routes its egress
through a local proxy that applies the network policy, and returns the command's
output and exit status to the caller.

## Types

- `PolicyExecConfig` holds the policy: the workspace, the filesystem view and
  `FsPolicy`, the `NetworkPolicy`, the credentials to mask and the helper
  binaries. `PolicyExecBuilder` turns it into a running `PolicyExec`.
- `PolicyExec` runs commands. `CommandSpec` describes one command and
  `CommandOverrides` narrows the session policy for it (`FilesystemOverrides`,
  `NetworkOverrides`, `CommandNetworkLists`). `RunningCommand` is a command in
  flight and `CommandOutput` with `CommandExit` is what it returns.
- `SandboxSettings` reads the settings document (`~/.sandbox-settings.json`) into
  a `PolicyExecConfig`. An option this crate cannot enforce is rejected with its
  path, never accepted and ignored.
- `AskCallback`, `RequestCallback` and `ResponseHook` let the caller decide
  connections and requests the policy does not settle, and edit request and
  response headers.
- `DependencyCheck` reports what the host lacks for the configured backend:
  errors stop a run, warnings name protections that will be weaker than asked.
- `proxy_only` serves an HTTP proxy to clients outside any sandbox and asks a
  decider on inherited streams about every request.
- On Windows, `WindowsLaunch` and `windows_exec_request` build a launch for
  `trogon-sandbox-fence-helper`, and `run_on_windows` runs it.

## Behavior

- A command the backend cannot confine as requested is refused, not weakened.
- Egress goes through the session proxy; a command sees only masked
  credentials, which the proxy replaces on the way out.
- On Linux the sandbox re-runs an executable for its network relay and file
  confiner. That executable calls `run_helper_if_invoked` first thing in `main`;
  `trogon-sandbox-policy-exec-helper` is one that does nothing else.

## Fit with the other sandbox crates

The policy types come from `trogon-sandbox-contract` and `trogon-sandbox-protocol`.
Isolation comes from `trogon-sandbox-subprocess-worker`, egress from
`trogon-sandbox-egress-proxy`, and the Windows network fence from
`trogon-sandbox-windows-fence`.
