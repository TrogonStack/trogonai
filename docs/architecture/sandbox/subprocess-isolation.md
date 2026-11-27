# Subprocess isolation

The `trogon-sandbox-subprocess-worker` (`rsworkspace/crates/sandbox/trogon-sandbox-subprocess-worker`) launches agent CLIs under bubblewrap (`bwrap`) on Linux. Isolation is pluggable through the `IsolationBackend` trait, so another confinement mechanism can be added without rewriting lease handling.

## Why bubblewrap

| Option | Pros | Cons |
|--------|------|------|
| bubblewrap (chosen) | User namespaces, no root. Ubiquitous on modern distros. Simple CLI. | Coarse egress (share or unshare the network only). |
| nsjail | Rich policy, cgroup hooks. | Heavier configuration, less common in base images. |
| firejail | Familiar profiles. | SUID concerns on some installs. |
| landlock | Fine-grained filesystem rules. | No network isolation on its own. Needs a recent kernel. |

The default Linux backend is `BubblewrapBackend`. The pass-through backend (`IsolationBackendKind::None`, selected with `none`) is allowed only when the lease does not request an isolation tier. The worker refuses a lease that asks for a tier the configured backend cannot provide.

## Egress

- `EgressPolicy::DenyAll`: `bwrap --unshare-all` without `--share-net`. The guest has private loopback networking without external connectivity.
- `EgressPolicy::AllowAll`: `--share-net` is added back for unrestricted host networking.
- Restricted policies use `EgressPolicy::Relay`: a private network namespace whose only way out is a relay to the lease's policy-enforcing egress proxy. Allowlists do not share the host network.

The worker's capability profile advertises `EgressEnforcementLevel::Mandatory`.

## Port previews

Linux bubblewrap leases with a private network support port previews, including servers started from interactive terminals. Each terminal retains its own private network namespace under the lease's network policy. A preview forwards client bytes only when exactly one current owned namespace accepts the port. If the main process and a terminal both listen on that port, the connection is refused until the ambiguity clears. Closing a terminal removes its namespace from routing and closes connections established through it. Removing the lease closes all its forwards.

Leases sharing the host network, macOS leases and the unconfined backend answer `Unsupported`, because their listening ports cannot be distinguished from other host services.

## Snapshots

Snapshot export and restore are out of scope for this provider. Its capability profile reports `can_snapshot`, `can_checkpoint_running` and `can_restore` as unsupported.

## Seccomp

The bubblewrap backend installs a seccomp filter that refuses by default:

- namespace and mount manipulation (`mount`, `umount2`, `pivot_root`, `setns` and the new mount API calls),
- creating user and mount namespaces through `clone` and `unshare`,
- `io_uring`,
- `AF_UNIX` and `AF_VSOCK` sockets.

Per-lease options (`allow_nested_user_namespaces` and `allow_all_unix_sockets` in the Linux options) relax the namespace and socket rules. The filter supports x86_64 and aarch64.

## Platform

This page covers the Linux `BubblewrapBackend`, whose implementation is gated with `cfg(target_os = "linux")`. The crate also includes other platform backends. For macOS seatbelt isolation, see [Run the subprocess worker on macOS](../../how-to/sandbox/run-the-subprocess-worker-on-macos.md).
