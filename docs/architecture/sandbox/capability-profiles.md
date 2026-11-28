# Provider capability profiles

This page explains why the sandbox stack describes each provider with a typed `ProviderCapabilityProfile` instead of hand-maintained capability bits, how `CapabilityRequirements` filters worker selection, and the honest defaults principle that keeps scheduling truthful. Broker authors, worker implementers and SDK consumers should read it before declaring or requesting provider capabilities.

The types live in `rsworkspace/crates/sandbox/trogon-sandbox-contract`. The broker's matching logic lives in `rsworkspace/crates/sandbox/trogon-sandbox-broker`.

## Why bitsets do not scale

A `WorkerCapabilities` bitset can gate coarse features such as snapshot, restore and S3 volumes. It cannot express:

- A warm pool size ("keep 5 idle containers per digest").
- Live checkpoint of a running sandbox.
- Named tag resolution.
- Per-tenant prewarm budgets.
- An egress enforcement level (`None`, `BestEffort` or `Mandatory`).

You cannot encode "warm pool of size 5" in a single bit, and adding a bit per feature leads to combinatorial growth and stale declarations. The fix is a structured profile per provider.

## `ProviderCapabilityProfile` shape

The profile is the source of truth. Each worker builds one for its provider and sends it in its `WorkerHeartbeat`, so the broker reads the declaration the running worker actually makes. Sub-structs partition concerns.

| Sub-struct | Examples |
|---|---|
| `SnapshotCapability` | `can_snapshot`, `can_restore`, `can_checkpoint_running`, `supported_artifact_types`, `layered_deltas`, `max_snapshot_size_bytes`, `supports_native_snapshots`, `supports_in_place_rollback` |
| `VolumeCapability` | `supported_modes` (a list of `VolumeMode`), `supports_s3_backend`, `supports_fuse_mounts` |
| `IsolationCapability` | `default_tier`, `supported_tiers`, `egress_enforcement`, `egress`, and many `supports_*` flags such as `supports_privileged`, `supports_sandbox_users`, `supports_interactive_stdin`, `supports_terminal`, `supports_output_redaction`, `supports_hardening_overrides` |
| `LifecycleCapability` | `supports_warm_pool`, `max_warm_pool_size`, `supports_prewarm_per_tenant`, `supports_named_tags`, `supports_pause_resume`, `supports_native_auto_stop`, `max_lease_lifetime_seconds`, `dormancy` |
| `PerformanceProfile` | `max_concurrent_sandboxes` |
| `ResourceCapability` | `vcpu_millis`, `disk_mib`, `reports_usage_gauges` |
| `PlacementCapability` | `regions`, `supports_native_passthrough`, `env_digest` |

Inspect a provider's profile with the admin tool:

```bash
trogon-sandbox-admin capability show docker
trogon-sandbox-admin capability diff docker hyperlight
```

## Egress enforcement

`EgressEnforcementLevel` is ordered `None < BestEffort < Mandatory`, and the level must describe what launch really does with `spec.network_policy`. `None` means the worker ignores it.

A requirement names a floor with `CapabilityRequirements.min_egress_enforcement`, and `satisfies` rejects any profile below it. The broker derives the floor from the lease's network policy (`NetworkPolicy::min_egress_enforcement`). `DenyAll` and `UserDefined` need at least `BestEffort`, `AllowAll` needs nothing, and a caller may ask for `Mandatory` explicitly through `required_capabilities`. A `UserDefined` policy also sets `needs_domain_allowlist`. A restricting policy is refused with `LeaseError::NoWorkerSatisfiesCapabilities` when no worker qualifies.

`EgressCapability` says which shapes of policy the worker really applies, one flag each:

- `deny_all`: `DenyAll` cuts all traffic.
- `host_allowlist`: a `UserDefined` policy can name hosts.
- `cidr_allowlist`: a `UserDefined` policy of IP ranges only is honored.
- `proxy_mediated`: traffic passes through a proxy this stack runs, not only a provider or kernel firewall.

All flags default to unsupported, so a profile from an older worker claims nothing new. A policy that names no hosts, only `allow_ranges` or `deny_ranges`, sets `needs_cidr_allowlist` instead of `needs_domain_allowlist`. `satisfies` accepts it from a worker with `egress.cidr_allowlist` or with `supports_domain_allowlist`, because a hostname proxy always handles ranges too. That is how a provider firewall that takes CIDR blocks and no hostnames (Daytona) can serve range policies while still being refused hostname ones.

Use `trogon-sandbox-admin capability show <provider>` for the values each worker declares. The [provider option matrix](../../reference/sandbox/provider-option-matrix.md) lists known gaps the flags do not capture.

### Where the worker enforces what it advertises

The broker gate is the first line, not the only one. A worker that is reached directly, or through a profile that has gone stale, must not serve a policy it cannot hold. Workers therefore refuse shapes outside their declared capability with a typed `LeaseError::InvalidNetworkPolicy` instead of launching with looser egress.

- Kubernetes, domain-only `UserDefined`: rules decided by hostname are enforced by an egress proxy sidecar that shares the pod's network namespace with the payload. A Kubernetes `NetworkPolicy` built from a policy with no IP ranges would have an empty egress list, so the proxy could not reach DNS or any upstream. Opening the `NetworkPolicy` to let the proxy through would also open the traffic the proxy never sees. The worker therefore refuses these policies and advertises `host_allowlist` and `proxy_mediated` as unsupported. `DenyAll`, `AllowAll` and policies made only of IP ranges are served through the `NetworkPolicy` alone.
- Docker, warm pool: only a lease with no policy or a closed network is served from the warm pool.
- Docker, `UserDefined`: the egress sidecar shares the workload's network namespace and installs a firewall before the proxy starts. Workload TCP is redirected into the proxy; everything else is rejected, IPv4 and IPv6 alike, including DNS, any other UDP (QUIC, DNS tunnels) and ICMP. Only the proxy's own uid may send DNS queries. Names therefore reach the network only through the proxy: the workload's `HTTP_PROXY` and `HTTPS_PROXY` point at the sidecar's explicit listener, which resolves the name itself, and a client that ignores those variables cannot resolve names at all. The launch waits for the sidecar to confirm the firewall and tears the sandbox down if it does not, so a failed rule never leaves a workload with open egress. The level stays `BestEffort`: an image entrypoint (`EntrypointDefault` boot) runs before the sidecar exists, and the firewall lives in a namespace the workload shares.
- Subprocess, `none` backend: the backend spawns a plain host process with the host network. Its profile declares no egress enforcement, so the broker does not route a `DenyAll` or `UserDefined` lease to it, and the worker refuses those policies itself.
- Modal: Modal has one switch, `block_network`. The worker refuses `UserDefined` rather than launching it with the switch off.
- Daytona: the profile is `BestEffort` because the provider firewall cannot be confirmed from the worker. A lease that requires `Mandatory` enforcement is refused by the capability gate.

The egress check applies whether or not `enforce_worker_registry` is on. With the registry off, the broker still consults heartbeating workers, but only for the egress fields (`needs_domain_allowlist`, `needs_cidr_allowlist`, `min_egress_enforcement`). Every other requirement stays advisory. A `DenyAll` or `UserDefined` lease therefore needs at least one worker heartbeating a qualifying profile even in that mode, because with no profile to read the broker cannot show the policy will be honored and fails closed. Leases without a restricting policy are unaffected.

## Feature flags and their requirements

Each feature that varies by provider has a profile flag and a matching `needs_*` field in `CapabilityRequirements`. `satisfies` rejects a provider whose flag is unsupported.

- `supports_interactive_stdin` gates interactive stdin on detached commands (`needs_interactive_stdin`).
- `supports_file_ops`, `supports_file_search` and `supports_file_archive` gate the structured file operations carried by the `file_op` verb. Workers without them answer the verb with a typed `Unsupported` reply for leases they own, so a caller who skips the requirement flags gets a clear refusal instead of a timeout.
- `supports_change_tracking` and `supports_file_watch` gate change tracking and file watching, which ride the same `file_op` verb. They are independent. The broker refuses these operations with a typed `Unsupported` reply when the provider profile lacks the matching flag, before dispatching to the worker.
- `supports_named_processes` and `supports_process_readiness` gate the `process` field of a detached `RunCommandRequest` and the `wait_ready` verb.
- `supports_graceful_stop` and `supports_command_restart` gate the `stop_command` and `restart_command` verbs. A stop sends SIGTERM, waits up to `grace_seconds` (default 10, capped at 300), then sends SIGKILL. A restart stops the execution the same way and runs it again from the request it started with, under a new execution id.
- `supports_terminal` gates the `terminal` verb, an interactive pseudo terminal with open, input, resize and close. See [Open an interactive terminal](../../how-to/sandbox/open-an-interactive-terminal.md).
- `supports_port_preview` gates `GetSandboxDomain`. A `LeaseRequest` that declares `exposed_ports` sets `needs_port_preview`. `supports_expose_after_create` says the worker also resolves ports that were not declared at create.
- `supports_hardening_overrides` gates the per-lease loosening of container hardening in `SandboxSpec.platform.container`. Setting any override adds `needs_hardening_overrides`. The broker also refuses the request unless tenant policy allows it. The policy denies every tenant by default, and operators list tenants in `SANDBOX_HARDENING_OVERRIDE_TENANTS` (comma separated).
- `supports_output_redaction` gates `RunCommandRequest.redact_values`, the opt-in list of literal values a worker replaces with `[REDACTED]` in stdout, stderr and error text before the output is streamed, capped or retained. The broker checks it on every `run_command` that carries values, even when `enforce_worker_registry` is off, and denies the command when the provider's profile is missing or false. Limits: exact bytes only, so an encoded or transformed copy is not caught, and at most 64 values of 8 KiB each.
- `reports_usage_gauges` (on `ResourceCapability`) says the worker can answer a usage query with cpu and memory gauges it measured itself. The per-lease counters are kept by the broker and need no worker support. A worker that does not answer leaves `gauges` absent rather than reporting zeros.
- `supports_network_policy_update` says the worker applies `update_network_policy` to a running lease in place. `supports_sandbox_groups` says the worker serves `create_group` and group membership changes. When the worker registry is enforced, the broker checks each verb against the owning provider's profile and refuses it with `NoWorkerSatisfiesCapabilities` before touching the lease record or the worker.
- `supports_native_auto_stop` (on `LifecycleCapability`) says the provider stops an idle sandbox on its own and the worker maps `SandboxSpec.idle_timeout_seconds` onto that setting. The broker's idle reaper enforces the idle timeout for all leases regardless of the flag.

There is no profile flag for durable logs: `read_logs` is served by the broker from its own JetStream stream, so it depends on the broker deployment. See [Stream command output](../../how-to/sandbox/stream-command-output.md).

## Lease lifetime

The broker caps a lease's total lifetime, the initial `ttl_seconds` plus every extension, at `SANDBOX_MAX_LEASE_LIFETIME_SECONDS` (default 86400). A request over the cap is denied with `would_exceed_max_lifetime` before any worker is asked, and the idle timeout may not exceed it either. Raising it is an operator decision, because it only helps where the provider behind the lease can hold a sandbox that long.

`SandboxSpec.no_ttl` asks for a lease with no wall-clock deadline. The broker refuses it unless the operator sets `SANDBOX_ALLOW_NO_TTL_LEASES=true`, and always refuses it without `idle_timeout_seconds`, so every such lease still ends on its own once idle. Its quota reservation is the operator's lifetime cap rather than `ttl_seconds`. A tenant whose wall cap (`SANDBOX_DEFAULT_WALL_CAP`) sits below the lifetime cap is refused with `quota_exceeded`, so an operator who enables `no_ttl` raises the wall cap with it.

`LifecycleCapability.max_lease_lifetime_seconds` is a provider's own cap on one lease. `None` means no provider cap. A requirement's `lease_lifetime_seconds` must fit under it, and a `no_ttl` lease asks for `u32::MAX`, so it is never placed on a capped provider.

## Features advertised on a grant

`ProviderCapabilityProfile::sdk_features()` turns the profile into the client-visible feature names a `LeaseGranted` carries in `capabilities`. The names are the `SdkFeature` enum in `trogon-sandbox-contract`, and the TypeScript client's `SANDBOX_FEATURES` list (in `tsworkspace/packages/sandbox-ws-client`) must know each of them so `Sandbox.supports()` and `withCapabilities` can type them.

Before returning a grant, the broker clears any previously stored capability list and projects features from the current running lease record. When the record names a worker, only that exact worker's current, non-stale heartbeat can supply its profile. A missing, stale or mismatched owner profile contributes no worker features, and another worker of the same provider cannot supply them on its behalf. This also prevents an idempotent replay or named reconnect from preserving outdated feature claims.

Provider-wide profile fallback is limited to genuinely unassigned legacy records, where `worker_id`, `dispatch_attempt` and `epoch` are all absent.

`read_logs` is broker-owned and is added only when the broker's log store is configured, independently of the worker profile.

## Placement and native passthrough

`PlacementCapability` carries two declarations that the portable request cannot express on its own.

- `regions` lists where the worker can create a sandbox. A lease that names a region (`CapabilityRequirements.region`) is only eligible on workers listing it, and one that names none ignores the field.
- `supports_native_passthrough` says the worker validates and applies a `native` payload. A lease carrying `native` sets `needs_native_passthrough`, so it never reaches a worker that would silently drop the options.

The tenant policy gate for passthrough (default deny) is a broker decision and is not part of the profile. See [Native passthrough and regions](../../how-to/sandbox/native-passthrough-and-regions.md).

## Machine size ranges

A profile advertises the machine sizes it can enforce under `resources`, as `vcpu_millis` (thousandths of a vCPU) and `disk_mib`. `None` means the worker cannot enforce that dimension. A lease that sets `SandboxSpec.vcpu_millis` or `disk_mib` becomes a requirement, and `satisfies` passes only when the advertised range contains the requested value. When no registered worker qualifies, the broker refuses with `NoWorkerSatisfiesCapabilities`, so a size is never silently ignored.

vCPU is an integer in thousandths so that `SandboxSpec` stays `Eq` while still expressing the fractional values Docker and Kubernetes accept. The Docker worker advertises disk only when the operator opts in with `--disk-quota-max-mib`, because the storage option it relies on requires a quota-capable storage driver.

## Worker selection flow

When the broker receives a `LeaseRequest`:

1. It reads `required_capabilities`, if present.
2. It filters registered workers whose profile satisfies the requirements.
3. Among survivors, it walks `provider_pref` order.
4. It prefers workers whose `WorkerHeartbeat.cached_digests` contains the requested snapshot digest.
5. If no worker satisfies the requirements, it returns a typed error rather than falling back silently to a weaker provider.

## `WorkerCapabilities` is derived, not declared

The `WorkerCapabilities` bitset remains as a fast-path summary for hot scheduling paths, but workers do not set it independently. It is built from the profile with `WorkerCapabilities::from(&profile)`: `SUPPORTS_SNAPSHOT` follows `snapshot.can_snapshot`, `SUPPORTS_RESTORE` follows `snapshot.can_restore` and `SUPPORTS_S3_VOLUMES` follows `volume.supports_s3_backend`, among others. If the profile says restore is unsupported, the bitset cannot advertise it.

## Honest defaults principle

A provider's profile describes what it can do today, not what it might do next quarter. Every flag defaults to unsupported. A field flips to supported only when the underlying code ships and its acceptance test passes. Never pre-flip a flag because an implementation is planned, because scheduling trust depends on profiles being truthful. For example, the subprocess worker reports snapshot, checkpoint and restore as unsupported because snapshots are out of scope for it.

## Relationship to isolation tiers

`IsolationTier` remains the user-facing isolation declaration. The profile's `IsolationCapability` lists which tiers a provider actually supports (`supported_tiers`) and its `default_tier`. The broker intersects tier eligibility, capability requirements and `provider_pref`. See [Isolation tiers and runtimes](./isolation-runtimes.md) for the tier-to-runtime mapping.

## See also

- [Snapshot store](./snapshot-store.md): snapshot restore requirements.
- [Volumes](./volumes.md): `VolumeCapability`.
- [Isolation tiers and runtimes](./isolation-runtimes.md): isolation tiers.
- [Restore from a snapshot](../../how-to/sandbox/restore-from-snapshot.md): requesting restore.
- [Publish a snapshot](../../how-to/sandbox/publish-a-snapshot.md): requesting export.
- [Native passthrough and regions](../../how-to/sandbox/native-passthrough-and-regions.md): `PlacementCapability`.
- [Media types](../../reference/sandbox/media-types.md): the `ArtifactType` list.
