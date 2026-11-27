# Sandbox subprocess worker

`trogon-sandbox-subprocess-worker` is a sandbox worker that runs each lease as an
isolated process on the machine it runs on. It subscribes to lease dispatch over
NATS, launches the lease's command under an isolation backend, and serves the
lease's file operations, terminals, port forwards and snapshots. The
`trogon-sandbox-subprocess-worker` binary runs it on Linux, macOS and Windows.

## Types

- `SubprocessWorker` implements `trogon_sandbox_protocol::Provider` for Linux and
  macOS. `WorkerConfig` and `FuseConfig` configure it. On Windows,
  `WindowsWorker` and `WindowsWorkerConfig` play the same role.
- `IsolationBackend` is how a payload is confined, and `LaunchedProcess` the
  process it returns. The `isolation` module holds the backends: bubblewrap with
  seccomp and Landlock on Linux, a Seatbelt profile on macOS, and an AppContainer
  or a network-fenced account on Windows. `IsolationBackendKind` names them.
- `LaunchSpec`, `EgressPolicy` and `ExtraBind` describe one launch: the command,
  the filesystem policy, the network policy and the extra mounts.
- `lease_proxy`, `lease_secrets`, `proxy_env` and `tool_proxy_env` give each lease
  an egress proxy, masked credentials and the proxy and trust-store environment
  of common tools.
- `file_ops` serves the structured file verbs over a lease's workspace, and
  `WorkspaceRegistry` maps leases to their workspace roots.
- `terminal`, `port_forward` and `ssh_bridge` serve interactive terminals, local
  port forwards and the SSH bridge.
- `profile` reports the capabilities the worker advertises for its backend,
  `heartbeat` builds its heartbeat, and `admission` refuses a lease the backend
  cannot honor before anything is launched.

## Behavior

- A lease the backend cannot confine as requested is refused, not weakened.
- Egress goes through a per-lease proxy; the lease holds a token, never the real
  credentials, which are injected by the proxy.
- Anything a lease creates (directories, namespaces, accounts, ACLs) is removed
  with the lease.

## Fit with the other sandbox crates

The wire types come from `trogon-sandbox-contract` and the protocol behavior from
`trogon-sandbox-protocol`. The worker uses `trogon-sandbox-egress-proxy` for egress,
`trogon-sandbox-credential-broker` for credentials, `trogon-sandbox-file-ops` for
files, `trogon-sandbox-volumes` for FUSE mounts, `trogon-sandbox-ssh-gateway` for
SSH, and `trogon-sandbox-windows-fence` with `trogon-sandbox-fence-helper` for the
Windows network fence.
