# Provider option matrix

Every sandbox worker declares a `ProviderCapabilityProfile` and sends it with its heartbeat. The broker routes leases on those profiles: a lease that asks for a capability no live worker declares is refused with `NoWorkerSatisfiesCapabilities` before any worker sees it. This page says where to read the declarations and how to read them. [Capability profiles](../../architecture/sandbox/capability-profiles.md) explains why routing works this way.

## Where the matrix lives

The side-by-side tables of every worker's declared profile are generated from the code into `rsworkspace/crates/sandbox/trogon-sandbox-capability-matrix/README.md`. The `doc_in_sync` test in that crate fails when the committed tables differ from the profiles, so they cannot drift. Regenerate them after changing a profile, from `rsworkspace/`:

```bash
cargo run -p trogon-sandbox-capability-matrix -- --write
```

Without `--write` the command only checks, and exits non-zero when the tables are stale.

To read the profile a running worker actually advertised, ask the stack instead of the generated tables:

| Command | Prints |
|---|---|
| `trogon-sandbox-admin capability show <provider>` | The profile from a live heartbeat for that provider, or from the worker registry when none arrives in time, as JSON |
| `trogon-sandbox-admin capability diff <provider-a> <provider-b>` | The fields where two providers' live profiles differ |

When no profile is found, `show` prints `no live worker heartbeat received for provider '<provider>'` and `diff` fails with `no live worker heartbeat for <provider>`.

## How to read the generated tables

| Element | Meaning |
|---|---|
| Section (`ingress`, `isolation`, `lifecycle`, `performance`, `placement`, `provider`, `resources`, `snapshot`, `volume`) | One top-level group of `ProviderCapabilityProfile` |
| Row | One leaf of the profile, as a dotted path, for example `egress.deny_all` under `isolation` |
| Column | One worker, rendered at the defaults its binary documents |
| `true` / `false` | The worker declares, or does not declare, the capability |
| A value in backticks | The declared value, for example a tier, a limit or an artifact type |
| `none` | An empty list or an explicit "no support" variant |
| `-` | The field is absent from that worker's profile |
| `platform-dependent` | The subprocess worker decides the value at startup from the host operating system or by probing it (cgroup controllers, Seatbelt, AppContainer), so it differs between Linux and macOS |

Columns that need operator input are rendered with fixed values: `firecracker (networked)` is the firecracker profile with networking turned on, and `fly` is rendered in region `iad` because `--region` has no default. A worker started with other flags can declare a different profile; `capability show` reports what it really sent.

## Profile groups

| Group | What it declares | Read by |
|---|---|---|
| `provider` | The `ProviderKind` the profile belongs to | Dispatch subject selection |
| `snapshot` | Whether the worker can snapshot, checkpoint a running sandbox, restore, and which artifact types it produces | Snapshot and restore admission |
| `volume` | Supported mount modes, FUSE mounts, S3-backed volumes | Mount admission |
| `isolation` | Isolation tiers, egress controls and their enforcement level, and per-feature flags (`supports_terminal`, `supports_file_ops`, `supports_interactive_stdin`, `supports_port_preview` and others) | Lease admission and per-verb checks |
| `lifecycle` | Warm pools, pause and resume, dormancy modes, named tags, maximum lease lifetime | Lease admission and idle handling |
| `performance` | Expected cold start and restore times, concurrent sandbox capacity | Placement among eligible workers |
| `resources` | Accepted `vcpu_millis` and `disk_mib` ranges, usage gauges | Size admission |
| `placement` | Native passthrough support and declared regions | `native`, `region` and `fallback_regions` admission |
| `ingress` | Ingress mode and the maximum number of exposed ports | `exposed_ports` admission |

## Size fields

`vcpu_millis` and `disk_mib` on `SandboxSpec` are optional. A lease that sets one is only eligible for workers whose `resources` group declares a range that contains it; otherwise the broker refuses it with `NoWorkerSatisfiesCapabilities`. On the docker worker, a lease that carries a size skips the warm pool.

## Docker worker container hardening

The docker worker applies these defaults to every sandbox container. They are worker settings, not lease fields, so a tenant cannot loosen them through the spec. `privileged` stays rejected and `supports_privileged` stays `false`.

| Setting | Default | Flag and env var |
|---|---|---|
| Capabilities | All dropped, then `CHOWN`, `DAC_OVERRIDE`, `FOWNER`, `FSETID`, `KILL`, `SETGID` and `SETUID` added back | `--keep-default-capabilities`, `SANDBOX_DOCKER_KEEP_DEFAULT_CAPABILITIES` |
| `no-new-privileges` | On | `--allow-new-privileges`, `SANDBOX_DOCKER_ALLOW_NEW_PRIVILEGES` |
| Process limit | 4096 (`0` disables it) | `--pids-limit`, `SANDBOX_DOCKER_PIDS_LIMIT` |
| Swap | Pinned to the memory limit | `--allow-swap`, `SANDBOX_DOCKER_ALLOW_SWAP` |
| `/dev/shm` size | 256 MiB | `--shm-mib`, `SANDBOX_DOCKER_SHM_MIB` |
| Read-only root filesystem | Off; when on, writable tmpfs at `/tmp` and `/run` | `--readonly-rootfs`, `SANDBOX_DOCKER_READONLY_ROOTFS` |
| OCI runtime | The Docker daemon default, for example `runsc` or `kata` when set | `--runtime`, `SANDBOX_DOCKER_RUNTIME` |
| User | The lease's `user`, else this setting, else the image's | `--default-user`, `SANDBOX_DOCKER_DEFAULT_USER` |

The egress sidecar keeps only `NET_ADMIN`, `SETUID`, `SETGID` and `SETPCAP`, which it needs to install its rules and drop to the proxy user.

For a `UserDefined` policy the sidecar's firewall, in the network namespace it shares with the workload, allows:

| Traffic | IPv4 | IPv6 |
| --- | --- | --- |
| Workload TCP | Redirected to the proxy | Rejected |
| Workload UDP, DNS included, and ICMP | Rejected | Rejected |
| Proxy uid TCP, and UDP to port 53 | Allowed | Allowed |
| Loopback to `127.0.0.1` / `::1` | Allowed | Allowed |
| Replies on connections opened from outside (published ports) | Allowed | Allowed |

The workload resolves names through the proxy: its `HTTP_PROXY`, `HTTPS_PROXY` (and lowercase forms) carry the sidecar's explicit listener at `127.0.0.1:3129` with a per-lease password, and `NO_PROXY` covers loopback. A launch whose sidecar does not confirm the firewall within 15 seconds fails, and both containers are killed. When the namespace has no IPv6 stack the IPv6 rules are skipped, since there is no IPv6 egress to refuse.

Per-lease overrides go in `platform.container` (`ContainerOverrides`: `pids_limit`, `writable_rootfs`, `extra_capabilities`, `shm_size_mib`). Only workers that declare `supports_hardening_overrides` accept them, and the broker denies them for every tenant unless the tenant is listed in `--hardening-override-tenants` (`SANDBOX_HARDENING_OVERRIDE_TENANTS`, comma separated).

## Related pages

- [Capability profiles](../../architecture/sandbox/capability-profiles.md)
- [Isolation runtimes](../../architecture/sandbox/isolation-runtimes.md)
- [Native passthrough and regions](../../how-to/sandbox/native-passthrough-and-regions.md)
