# Sandbox contract

`trogon-sandbox-contract` provides validated worker, dispatch-attempt, and operation
identifiers, lease identifiers and policies, artifact metadata, image and snapshot
references, drive contracts, operation kinds, and the operation state transition
rules.

`WorkerId`, `DispatchAttemptId`, and `OperationId` serialize as JSON strings. Each
accepts 1 to 128 bytes containing only ASCII letters, digits, `_`, or `-`.
Construction, parsing, and deserialization enforce the same grammar and return
typed validation errors.

`Digest` serializes as a JSON string containing `sha256:` followed by exactly 64
lowercase ASCII hexadecimal characters. Construction, parsing, and deserialization
enforce this grammar. `DigestError` distinguishes a missing prefix from an invalid
hash. Validation checks the identifier's form; it does not hash or verify artifact
content.

`ArtifactType` preserves the JSON names and media types below. These describe
artifact metadata; they do not establish provider or runtime support.

| JSON name | Media type |
| --- | --- |
| `sandbox_rootfs_v1` | `application/vnd.trogon.sandbox.rootfs.v1+tar` |
| `hyperlight_snapshot_v1` | `application/vnd.trogon.hyperlight.snapshot.v1` |
| `firecracker_snapshot_v1` | `application/vnd.trogon.firecracker.snapshot.v1` |
| `kubevirt_disk_v1` | `application/vnd.trogon.kubevirt.disk.v1+qcow2` |
| `sbom_v1` | `application/vnd.trogon.sbom.v1` |
| `scan_report_v1` | `application/vnd.trogon.scan-report.v1` |

`ProviderKind` preserves the provider identity used by sandbox wire messages.
Its JSON names are `k8s_micro_vm`, `subprocess`, `local_docker`, `daytona`,
`modal`, `hyperlight`, `v8_wasm`, `kube_virt`, `azure_container_apps`,
`substrate`, `firecracker`, `proxmox`, `fly`, `e2b`, `vercel`, `space`, and
`mock`. The enum records identity only; it does not select, start, or authorize
a provider.

`IsolationTier` names the isolation strength requested by a lease. Its JSON names
are `container`, `bwrap`, `user_space_kernel`, `micro_vm`, `full_vm`, and
`wasm_micro_vm`. The contract preserves the ordered provider mappings for each
tier and the inverse provider capabilities; it does not select or authorize a
provider.

Capability metadata uses `VolumeMode` values `read_only`, `read_write`, and
`copy_on_write`; `EgressEnforcementLevel` values `none`, `best_effort`, and
`mandatory`; and `ResourceLimitMechanism` values `none`, `rlimit`, `cgroup_v2`,
`job_object`, and `runtime`. `SnapshotCapability`, `VolumeCapability`, and
`EgressCapability` describe provider support and preserve their defaulted JSON
fields. Each support flag is a `CapabilitySupport` value, `Supported` or
`Unsupported`, serialized as a JSON boolean so existing peers keep parsing it;
an omitted flag reads as `Unsupported`. These values report capabilities; they
do not authorize or select a provider.

`ProviderCapabilityProfile` is the full profile a provider's worker declares:
snapshot, volume, isolation, lifecycle, and performance axes, plus resource
ranges, placement, and ingress limits that default when an older peer omits
them. `ResourceRange` rejects a minimum above its maximum. `CapabilityProfile` is the flattened tenant view derived from it, and
`ListCapabilitiesRequest` and `ListCapabilitiesResponse` carry those views.
`sdk_features` lists the `SdkFeature` values a profile vouches for, in a fixed
order, leaving out any feature without a backing flag. `WorkerCapabilities`
serializes as a `u32` bit set and rejects unknown bits. `DormancyCapability`
resolves a `SuspendRequestLevel` only to a suspend level the provider offers,
and `EnvDigest` reuses the `Digest` grammar. These profiles report capabilities;
they do not select, place, or authorize a provider.

`CapabilityRequirements` is what a lease asks of a worker: `needs_*` flags, minimum
tiers and levels, and optional limits such as region and lifetime. Each flag is a
`CapabilityNeed`, `Required` or `NotRequired`, serialized as a JSON boolean; an
omitted flag reads as `NotRequired`. The contract carries the request only. Matching
it against a profile is not part of the contract. `LeaseError` and `DriveError` are
tagged by `kind`, and `LeaseError::NoWorkerSatisfiesCapabilities` carries the
requirements that found no worker.

`LeaseId` and `CorrelationId` serialize as JSON strings and accept any string.
`TenantId` also serializes as a JSON string, but holds 1 to 128 bytes with no
control characters; construction, parsing, and deserialization all enforce it.
`EmptyObject` serializes as `{}` for enum variants that carry no fields, and
deserializes only from an empty JSON object.

`SandboxTags` serializes as a JSON object of string values and rejects more than
five tags on construction and deserialization. `contains_all` matches a tag
filter, and an empty filter matches every lease.

`IdlePolicy` lists `IdleStage` values, each applying an `IdleAction` (`pause`,
`suspend`, or `release`) once a lease has been quiet for more than a nonzero
`after_seconds`. A policy holds at least one stage, strictly increasing stage
times, and strictly more severe actions; construction and deserialization both
enforce it. `fits_within` reports whether every stage fires within an
operator's ceiling on idle time.

`SnapshotPolicy` names the `ArtifactType` and target repository for a lease's
snapshots, plus an optional `RetentionPolicy`, `PrewarmSpec`, and tag name that
are omitted from JSON when absent. A `RetentionPolicy` keeps `max_tags` as a
`RetentionTagLimit` between 1 and 10; construction and deserialization both
enforce the bounds. `delete_evicted` is an `EvictedArtifactAction`, `Delete`
or `Keep`, serialized as a JSON boolean; an omitted value reads as `Keep`, which
leaves the evicted artifact in the registry. These policies describe intent;
they do not schedule idle handling, take snapshots, or prune a registry.

`OciImageRef` splits an image into `registry`, `repository`, and an optional
`tag` and `digest`, which are omitted from JSON when absent.
`OciImageRef::parse_unqualified` treats the first path component as the registry
only when it contains `.` or `:`, or is `localhost`, and splits off an `@` suffix
as the digest only when it is a valid `Digest`; an invalid suffix stays in the
reference. `pattern_matches_legacy` matches an
allowlist pattern exactly or by a single trailing `*`; an empty pattern matches
nothing. `pattern_matches_oci_image_ref` also compares a pattern part by part, so an
empty registry, an absent tag, or a `*` tag matches any value, and a pinned digest
matches only the same digest. `OciArtifactRef`
pins an `ArtifactType` by digest in a registry.

`NativeSnapshotRef` names a snapshot a provider keeps itself, scoped to its
`ProviderKind`, with an optional `expires_at` in Unix seconds. Its
`NativeSnapshotId` holds 1 to 128 bytes of ASCII letters, digits, `-`, `_`, or
`.`; construction, parsing, and deserialization all enforce it.
`NativeSnapshotReceipt` carries the reference and an optional size.
`PullSecretId` and `ServiceAccountId` serialize as JSON strings and accept any
string; a `PullSecretId` names a secret, it never carries one.

`SnapshotTag` scopes a `SnapshotTagName` to a `TenantId`. A `SnapshotTagName`
holds 1 to 64 bytes of ASCII letters, digits, `.`, `_`, or `-`; construction,
parsing, and deserialization all enforce it. `SnapshotTagRecord` and
`SnapshotTagAuditEvent` (tagged by `event` as `set` or `cleared`) record who moved
a tag and when, using serde's `SystemTime` encoding of `secs_since_epoch` and
`nanos_since_epoch`.

`DriveId` scopes a `DriveName` to a `TenantId`; a `DriveName` holds 1 to 64
bytes without `/`, `\`, or `..`. `DriveMount` mounts a drive at a `MountPath` in a
`DriveMountMode`, `read_write` by default or `read_only`. A `MountPath` is
absolute, does not resolve to `/`, such as `/.` or `//`, and has no `..`
component. Construction, parsing, and deserialization enforce both rules.
`DriveStorage` is untagged: an S3 bucket or a `ProviderVolumeRef`, whose `ProviderVolumeId` rejects an empty
value, more than 256 bytes, whitespace, or control characters. `DriveRecord`
flattens its storage into the record, and `DriveRecord::DEFAULT_SIZE_GIB` (100)
and `DriveRecord::MAX_SIZE_GIB` (1024) bound a requested size. `DriveError` is
tagged by `kind`. `SandboxUserName` and `SandboxGroupName` accept only
`^[a-z_][a-z0-9_-]*$` within 32 bytes, on construction, parsing, and
deserialization. `CreateSandboxUserRequest`, `CreateSandboxGroupRequest`, and
`ModifyGroupMembershipRequest` name a lease and those validated names, and their
responses are tagged by `outcome` as `created`, `already_exists`,
`missing_dependency`, `applied`, or `denied`. `GroupMembershipAction` is `add` or
`remove`. These contracts describe drives and accounts; they do not
provision storage, mount volumes, or create users.

`WorkerHeartbeat` is the periodic report a worker publishes: its `WorkerId`,
`ProviderKind`, full `ProviderCapabilityProfile`, `WorkerCapacity`, cached and
warm-pool image digests, and the `NativeVolumeBinding` values it holds.
`WorkerDispatchProtocol` names the dispatch format the worker accepts, `legacy` by
default or `targeted_v1`, and `supports_lease_epoch` is a `CapabilitySupport`
flag. Fields added after the first heartbeat default when an older worker omits
them, and empty volume bindings and warm-pool counts are left out of the JSON. A
heartbeat reports state; it does not register a worker or route a lease.

`EnvironmentDefinition` names what makes one execution environment differ from
another: its `ProviderKind`, architecture, pinned `components` (a sorted map), boot
arguments, snapshot format, isolation profile, and optional CPU template, stamped
with `EnvironmentDefinition::SCHEMA_VERSION`. `EnvDigest` carries the manifest
annotation key as `EnvDigest::ANNOTATION_KEY`. `LegacySnapshotPolicy` is
`allow_with_warning` by default, or `refuse`, for snapshots that record no digest.
The contract does not hash a definition into a digest or decide whether a restore
may run.
`SnapshotKvRecord` is the catalog entry a snapshot builder writes, with optional
`parent`, `created_at` in serde's `SystemTime` encoding, and `env_digest` that are
omitted when absent.

`WorkerAuthorityRequest` asks the broker which of a worker's `LeaseId` values it
still owns. `WorkerAuthorityResponse` lists the `OwnedLease` values, each with an
optional `LeaseEpoch` omitted when absent, and the `not_assigned` leases, which
default to empty. These types carry the question and answer; they do not check
ownership or freeze a lease.

`LeaseStatus` uses the JSON strings `pending`, `running`, `paused`, `suspending`,
`suspended`, `waking`, `cancelling`, `released`, and `failed`. `LeaseUsage`
reports one lease's command and file counters, its running, paused, and suspended
seconds, and optional `ResourceGauges`; `TenantUsage` sums the same counters
across a tenant's leases. `LeaseCounters` holds the totals a lease accrues
while it runs; `add` saturates at `u64::MAX` and leaves the pause and suspend
bookkeeping untouched. `AccountLimits` pairs a
tenant's usage with its caps, and `TagLimits` adds an optional cap on active
leases per tag value. `ResourceUsageRequest`, `GetLeaseUsageRequest`,
`GetTenantUsageRequest`, and `GetAccountLimitsRequest` carry the identifiers for
those reads. `ResourceUsageResponse`, `GetLeaseUsageResponse`,
`GetTenantUsageResponse`, and `GetAccountLimitsResponse` answer them, tagged by
`outcome` as `gauges`, `unsupported`, `usage`, `limits`, or `denied` with a
`LeaseError`. These types report usage; they do not meter, bill, or enforce a
quota.

The lease lifecycle messages cover stop, cancel, pause, resume, rollback, suspend,
and wake. Each client request (`StopLeaseRequest`, `PauseLeaseRequest`,
`ResumeLeaseRequest`, `RollbackLeaseRequest`, `SuspendLeaseRequest`,
`WakeLeaseRequest`) names a tenant and a lease, and most take an optional
`OperationId`. Each reply is tagged by `outcome` and carries `Denied` with a
`LeaseError`; pause, resume, rollback, and the worker replies also have
`unsupported`. `CancelIntent` is `release` or `purge`. The worker messages
(`SandboxFreezeRequest`, `SandboxRollbackRequest`, `SandboxSuspendRequest`,
`SandboxWakeRequest` and their responses) are broadcast per provider. A suspend
keeps a `DormancyRecord`, whose `DormancyArtifact` is `local` to a worker or
`remote` in a registry. These types only describe the wire. Clamping budgets,
choosing a suspend level, and routing are left to the broker.

`SnapshotPublished` and `SnapshotDeleted` announce a snapshot entering or leaving
the registry, and `LiveSnapshotEntry` is one the tenant is charged for. Each may
carry a `SnapshotGeneration`, a canonical ULID of 26 uppercase Crockford base32
characters starting with `0` through `7`; construction, parsing, and
deserialization all enforce it. `SnapshotPublishedTag` names the
`SnapshotTagName` a builder moves to the snapshot and the `set_at` time tag moves
are ordered by. `SnapshotAccountingResponse` is tagged by `outcome`: `committed`
with a `SnapshotAccountingDisposition` of `live`, `superseded`, or `deleted`, or
`refused` with a `SnapshotAccountingRefusal`. `SnapshotCaptureStatus` is tagged
by `kind` as `created` or `failed`, and `SnapshotRestoreResult` carries the
digest a worker restored. `LeaseFailed` reports a `LeaseFailedReason`, tagged by
`kind`; its `digest_mismatch` holds a `DigestMismatch` whose `expected` and
`actual` digests differ, which construction and deserialization both enforce.
These events describe snapshots; they do not push, delete, or charge for
artifacts.

The snapshot API carries the client-facing requests and responses for snapshots.
Lookup, lineage, tag, and delete requests name a tenant and a `Digest` or a
`SnapshotTagName`. Each response is tagged by `outcome`, and a refusal arrives as
`denied` with a `LeaseError`, so it is never mistaken for `not_found`.
`SnapshotTreeResponse` and `SnapshotTagHistoryResponse` carry an optional `denied`
field that older brokers never send. `SnapshotLeaseRequest` captures a running
lease. Its `accept_native` flag is a `NativeSnapshotAcceptance`, a JSON boolean
that reads as `Declined` when omitted. `SnapshotLeaseResponse` returns an OCI
`LeaseSnapshotReceipt` or a `NativeSnapshotReceipt`. `SnapshotPushTarget`,
`SnapshotExportRequest`, `CheckpointRunningRequest`, and `SnapshotExportError`
carry the worker-side export. These types describe the exchange; they do not
export, delete, or authorize anything.

The lease operations carry the other lease and drive requests. They cover fork,
extend, tag update, list, delete by lease or by name, owned cleanup, and drive
create, list, and delete. Lease names in by-name requests, lease rows, and name conflicts are a `LeaseName`, 1 to 128 bytes of text, enforced on construction, parsing, and deserialization. Each response is tagged by `outcome`. `ReapLeasesRequest`
releases leases that match its filters, and its `dry_run` field is a `ReapMode`,
a JSON boolean that reads as `Release` when omitted. A reap needs at least one of `tags`,
`older_than_seconds`, or `idle_for_seconds`; construction and deserialization
both enforce it with a `ReapLeasesRequestError`. The tag filters are `SandboxTags`. Page size, drive size, and lease lifetime limits
are broker policy and are not part of these types.

`WorkspaceSource` is keyed by its variant name: `clone` checks out a Git `sha`
with `CloneAuth`, `host_mount` mounts a host directory, `s3_volume` mounts an S3
prefix in a `VolumeMode` with an optional seed snapshot, and `empty` starts from
nothing. A host mount's `HostMountAccess` serializes as the JSON boolean
`readonly`, and an omitted value reads as `ReadWrite`. `CloneAuth` is `none`,
`bearer_token`, or `basic`, and each secret is a `SecretRef`, either a NATS
key-value entry or a literal. `Debug` and `Display` never show a literal value,
but serialization carries it to the worker. These types describe where a
workspace comes from; they do not clone, mount, or fetch secrets.

`CommandReadinessRequest` asks a worker whether a lease's sandbox can run
commands through the sealed supervisor. It binds the probe to a tenant, a lease,
a `ProviderKind`, a `CommandReadinessTarget`, and a `SandboxId` of 1 to 512
bytes with no control characters. The target's `LeaseEpoch` is required and is
never zero, and the lease id must be 1 to 1024 bytes of dot-separated tokens
with no whitespace, control characters, `*`, or `>`. Construction and
deserialization both enforce these rules and reject unknown fields.
`CommandReadinessResponse` is tagged by `outcome`: `ready` with a
`CommandReadinessRuntime`, whose `sha256` is the bare hex hash of the
supervisor, `unsupported` or `unavailable` with the binding and a reason, or
`denied` with a reason. These types describe the probe; they do not run it.

`RunCommandRequest` asks a worker to run a command in a lease's sandbox, and its
`RunCommandResponse` is tagged by `outcome` as `completed`, `started`, or
`denied`. Options only a detached command may use, namely interactive stdin, a
lease deadline, and a `ProcessSpec`, live in `CommandExecution::Detached`, and
deserialization rejects them on an attached command. A `CommandEnv` cannot set
the reserved execution id variable, and `RedactValues` holds at most 64 values
of at most 8192 bytes each. A `LaunchKey` is 1 to 128
characters of `[A-Za-z0-9_-]`, a `ProcessName` is 1 to 64 characters of
`[A-Za-z0-9._-]`, a `ReadinessNeedle` is 1 to 4096 bytes, and `ReadyPorts` holds
at most 16 distinct nonzero ports. Each rule is enforced on construction and on
deserialization. A `RedactValue` never shows its value in `Debug` and has no
`Display`, but serialization carries it. Every wire boolean, such as
`detached`, `stream_output`, `timed_out`, and `eof`, is a two-variant enum that
still serializes as a JSON boolean. `UnixSignal` is a closed set of signal
names, so a free-form string never reaches a worker. A `WriteStdinRequest`
carries a `StdinChunk` of at most 64 KiB and must hold data or end the input.
The stop, restart, kill, cancel launch, poll, wait for readiness, list
processes, and read logs requests each have a tagged response. The file types
`WriteFilesRequest` and `ReadFileRequest` write and read files as an optional
`SandboxUserName`. These types describe the messages; they do not run commands,
trim output, or move bytes.

`DispatchTarget` names the worker and attempt a lease is dispatched to, with an
optional `LeaseEpoch` for workers that support epochs.

`LeaseAttemptIdentity` binds a lease, a `ProviderKind`, and a `DispatchTarget`.
`ProviderCleanupRequest` carries it with a `CancelIntent`, and
`ProviderCleanupResponse` is tagged by `outcome` as `cleaned` or `failed`.
`LeaseDispatchAttempt` records one attempt: its `DispatchAttemptId`, optional
`LeaseEpoch`, `LeaseRequestKey`, `LeaseAccounting` (`tenant` or `prewarm`), and
optional cleanup reason, cleanup start, and `PendingDispatch`, which keeps the
dispatch payload as opaque JSON until the reply commits. A `LeaseRequestKey`
holds 1 to 256 bytes of ASCII letters, digits, `_`, or `-`; construction,
parsing, and deserialization all enforce it. `AttemptQuotaReservation` names the
tenant, attempt, and CPU and wall seconds held for an attempt. `LeaseReleased`
announces a lease ending with a `LeaseReleasedReason` (`completed`, `expired`,
`cancelled`, `idle_timeout`, or `failed`). `IdlePauseRetry` and `FreezeInFlight`
carry the backoff and in-flight markers of idle pauses; `FreezeAction` is
`Pause` or `Resume`, serialized as the JSON boolean `thaw`. These types describe
the wire; they do not reserve quota, pick a worker, or drive a retry.

The credential broker contracts cover issuing, revoking, and rotating
credentials. `CredentialIssueRequest` asks for a token with a
`CredentialScope`, tagged by `kind`, and a nonzero `ttl_seconds`. It may carry a
`PolicyFence` naming the committed policy revision it was built from.
`CredentialIssueOutcome` and `CredentialAckOutcome` are tagged by `outcome` as
`ok` or `err`, and every `CredentialError` is tagged by `kind`. A
`CredentialAck` always serializes with `ok: true`, and deserialization rejects
`ok: false`. `ShortLivedToken` and `OidcToken` never show their value in
`Debug` and have no `Display`, but serialization carries it.
`CredentialAuditEvent` is tagged by `event` as `issued`, `revoked`, or
`rotated`, and `CredentialProvider` parses its wire names. These types describe
the broker's messages; they do not store, mint, or verify credentials.

The network egress contracts carry a lease's outbound policy and what the proxy
reports. `NetworkPolicy` is `allow_all`, `deny_all`, or `user_defined` with
`DomainRule` entries, `AllowRange` allow and `IpRange` deny lists, `DeniedDomain` patterns, an
`OpaqueTunnelPolicy`, and `RequestRule` entries. `OpaqueTunnelPolicy` is `refuse` unless the policy says
`"opaque_tunnels": "allow"`: an omitted field reads as `refuse`, and the field is always serialized so an older
reader never mistakes its absence for `allow`. An `IpRange` is CIDR notation; an `AllowRange` is a
CIDR range, a single address, or an address with a port that allows that destination on that port only. A `DomainPattern` is a host, a
`*.` wildcard, or `*`, with an optional `Port`; a `Host` is a lowercase DNS name
or an IP address in WHATWG form, and an IPv4-mapped IPv6 address reads as IPv4.
`HttpMethod`, `HeaderName`, `HeaderValue`, and `PathPrefix` bound and normalize
what a request rule names, and the matcher DSL and credential injection reuse
`HttpMethod` and `HeaderName`. A `ForwardUrl` is a URL with no query or
fragment, a `Base64Data` is standard base64, and a `RefusedRequest` cuts the
query and fragment of its target on deserialization too. `DeniedDomain` exposes
its `target` and `port` rather than a `DomainPattern`, and `PortAccess` and
`SshAccess` redact secrets in `Debug`. `DecideRequest` and `DecideResponse` carry a
connection verdict, and `RequestDecideRequest` and `RequestDecideResponse` carry
a per-request one, whose `Debug` shows header names but never values.
`EgressViolation` and `FsViolation` report a refusal, using a `SecretName` or a
canonical `LiteralPath`. `GetSandboxDomainRequest`, `NativeHandleRequest`, and
`SshAccessRequest` and their responses are tagged by `outcome`. Every flag on
these types is a two-variant enum, such as `BodyTruncation`, serialized as a
JSON boolean. These types describe messages and policy; they do not match a
request, enforce a rule, or open a connection.

The filesystem policy contracts carry what a sandbox may read and write. An
`FsPolicy` holds six lists of `FsPathEntry`: `deny_read`, `credential_deny_read`,
`allow_read_within_deny`, `own_allow_within_deny`, `allow_write`, and
`deny_write`. Each list holds at most 1024 entries. An entry is tagged by `kind`
as `literal`, a `LiteralPath` with an optional `DirectoryMarker`, or `glob`, a
`GlobPattern`. A `GlobPattern` is absolute, has at least one wildcard, and
refuses a `..` that follows a wildcard segment. It is a JSON string, or an
object with an `anchor` and a `tail` when the anchor is a plain name. A deny
glob that ends in a separator is refused, because it could match nothing. These
types describe the policy; they do not decide whether a path is allowed.

`PlatformOptions` carries the opt-ins only some platforms can enforce, in
`MacosOptions`, `LinuxOptions`, `FilesystemOptions`, and `ContainerOverrides`,
plus a list of git safe directories. Every group is left out of the wire when it
holds its defaults. Each flag is a two-variant enum, such as `PtyAccess`,
serialized as a JSON boolean. A `MachServiceName` allows only a single trailing
`*`. `ContainerOverrides` bounds a `PidsLimit` and a `ShmSizeMib`, and an
`ExtraCapability` is an upper case name that is never on the forbidden list.
A `MandatoryDenySearchDepth` is from 1 to 10.

`FileOpRequest` carries a `LeaseId`, an optional `SandboxUserName`, and one `FileOp`, a
structured file operation tagged by `op` as `list`, `stat`, `read`, `write`, `edit`,
`search`, `remove`, `mkdir`, `rename`, `archive_get`, `archive_put`, `checkpoint`,
`changes`, `watch_start`, `watch_poll`, `watch_stop`, or `checksum`. A worker answers
with a `FileOpResponse` tagged by `outcome`, whose failures carry a `FileOpErrorKind`.
Payload bytes are a `Base64Data` string, and a checksum is a
`FileSha256`, 64 lowercase hexadecimal characters. Every boolean option or result
flag, such as `recursive`, `truncated`, or `existed`, is a two-variant enum that still
serializes as a JSON boolean and keeps the same default when omitted. These types
describe the messages only; they do not read, write, search, or watch files, and the
size limits on `FileOp` are constants a worker enforces.

`NetworkOptions` gathers the optional egress proxy settings as public fields with everything off by default.
Its on/off flags are two-variant enums such as `LoopbackNames`, serialized as JSON booleans. `DenyReason`,
`TunnelCap`, `UnlistedDestinations`, `DeniedDomainReasons` (an object keyed by deny list entry, dropping keys that
are no entry), `MitmUnixSocket`, `UpstreamCaCertificates`, and `ResponseHeaderStrip` carry a JSON shape. The remaining
configuration types, among them `ParentProxy`, `ParentProxyUrl`, `NoProxy`, `CaFiles`, `TlsExclusions`,
`ExternalProxy`, and `RequestBodyCap`, are parsed on construction and have no JSON form. `ParentProxyUrl` keeps
URL userinfo as a `Basic` authorization value that `Debug` prints as `[REDACTED]`, and it has no `Display`. These
types do not match a host, resolve a proxy, or read the environment.

`OperationKind` uses the JSON strings `create`, `restore`, `fork`, `snapshot`,
`pause`, `resume`, `suspend`, `wake`, `stop`, `delete`, and `rollback`. A kind's
presence in the contract does not establish runtime support for that operation.

`OperationState` uses the JSON strings below. Transitions are allowed only to the
listed destinations:

| State | Allowed destinations |
| --- | --- |
| `queued` | `running`, `canceled`, `expired` |
| `running` | `running`, `queued`, `succeeded`, `failed`, `canceled` |
| `succeeded` | None |
| `failed` | None |
| `canceled` | None |
| `expired` | None |

The `running` self-transition permits takeover of a stale claim, while returning
to `queued` permits retry after a transient failure. Terminal states are immutable,
including self-transitions.

`SecretBinding` describes one credential the broker holds and the sandbox must never see in the clear: its
name, its target (an environment variable or a file), whether it is masked or denied, an optional extract rule,
the hosts it may be injected at, and an optional path, method, and port scope. The binding never holds the
credential value. It accepts only a coherent set of fields: a `deny` binding takes no extract rule, response header
stripping, or request scope, an environment variable target takes only a regex or jwt rule, and a no-match policy
needs an extract rule. `SecretBindings` holds at most 64 bindings with distinct names and targets, and
`AwsCredentialPairs` links masked bindings into an AWS credential. `ExtractPattern` accepts only an ECMAScript
regular expression with at least one capture group. Construction and deserialization enforce every rule, and the
JSON shape omits each defaulted field. Rules that need the sandbox spec, such as environment name collisions and
inject hosts that the network policy denies, are not checked here.

`SandboxSpec` is the wire description of one sandbox: its image, workspace, environment, CPU and memory limits, network policy, mounts, drives, exposed ports, secret bindings, idle policy, and retention toggles. Construction and deserialization enforce the rules that span fields: environment size and key limits, a single idle setting, no unbounded lease without an idle release, snapshot output only while snapshots are retained, distinct mount paths and exposed ports, at most four drives, and, for each secret binding, no environment collision, a user defined network policy, and inject hosts that an allowed domain covers and no denied domain fully covers. An omitted port on a version 2 binding means port 443. The `Debug` output shows environment names and redacts their values. Rules that need tenant policy or runtime state, such as quotas, image and snapshot existence, drive ownership, and provider capabilities, are not checked here.

`SandboxEntityId` is the 26 character Crockford base32 ULID of a persistent sandbox entity. It accepts either case, stores the uppercase form, and rejects the letters `I`, `L`, `O`, and `U`. `SandboxEntityRecord` is the durable record of an entity and `SandboxEntityRow` is the client-readable view without the spec template. `EntityState` serializes as a lowercase string for states with no payload and as a one-key object for states that carry a lease or a failure reason. `GetEntityRequest`, `ListEntitiesRequest`, and `DeleteEntityRequest` name an entity by `EntityRef`, and each response is tagged by `outcome`. `EntityLifecycleEvent` is the entity event stream payload, tagged by `event`. `LeaseGranted` and `LeaseResponse` carry the broker's reply to a lease request, with `created` kept as a JSON boolean, and `NetworkPolicyUpdateRequest` replaces the egress policy of a running lease. Entity transition rules are not part of this crate.

`LeaseRequest` is the tenant's request for a lease: tenant, provider preference, `SandboxSpec`, `ttl_seconds`, a `LeaseRequestKey`, and optional isolation tier, host imports, capability requirements, tags, `LeaseName`, opaque `native` provider JSON, `region`, `fallback_regions`, and `operation_id`. `prewarm` and `persistent` stay JSON booleans and are always written; an omitted `prewarm` reads as regular and an omitted `persistent` reads as persistent. `ProviderDispatchPayload` wraps a request with its lease id, correlation id, resolved drives, and dispatch target. `LeaseRecord` is the broker's durable lease record, where `sandbox_id` stays plain text because records written before the field existed hold an empty string.

`OperationRequest` is tagged by `kind` (`pause`, `resume`, `suspend`, `wake`, `delete`, `rollback`, `create`), `OperationResult` by `result`, `CreateQuotaStep` by `step`, and `GetOperationResponse` and `CancelOperationResponse` by `outcome`. `OperationRecord` carries the request, state, claim, allocation, compensation, result, and error of one durable operation. `OperationCapabilities.supports_create_cancellation` and `CancelOperationRequest.compensate_created` are JSON booleans, and an omitted `compensate_created` reads as false. Transition rules and request classification are not part of this crate.

`ProviderLease` is a worker's grant of a dispatch, and `ProviderDispatchReply` is untagged, told apart by `sandbox_id`, `provider_error`, `decode_error`, or `native_error`. `SnapshotReceipt` reports a snapshot export and `SnapshotExportOutcome` is tagged by `outcome` (`ok`, `native`, `err`). `DispatchAuth` is the envelope the broker adds to a dispatch, carrying the key id, operation, tenant, lease, optional worker, attempt, nonce, and epoch, an expiry in seconds, and the lowercase hex SHA-256 of the body. It does not sign or verify anything. `LifecycleEvent` is tagged by `status` and carries `SandboxPhase` values for phase progress.

This package defines values and allowed transitions. It does not execute
operations, persist state, authorize transitions, enforce claim ownership, or
provide worker, provider, transport, or lease-generation behavior.

`TerminalId` serializes as a JSON string of 1 to 64 ASCII letters, digits, `-`, or `_`, and `TerminalDimension` serializes as a JSON number from 1 to 1000. Both are enforced by construction and deserialization. `TerminalRequest` carries `lease_id` and `terminal_id` with a flattened `TerminalOp` tagged by `op` (`open`, `input`, `resize`, `close`). `TerminalResponse` is tagged by `outcome` (`opened`, `done`, `failed`, `unsupported`, `denied`) and wraps `TerminalError`, tagged by `kind`, or `LeaseError`. `TerminalOutput` frames are tagged by `type` (`data`, `exit`, `resync`). `TerminalInput` wraps `Base64Data` and caps the decoded write at 65536 bytes, refusing longer encoded text before decoding. `TerminalArgument` rejects NUL, and `open` carries a list of them as `cmd`. Output frames carry `Base64Data`. The contract does not buffer output or open terminals.

`SubstrateSizeClass` pairs a non-empty label with non-zero memory in MiB, and
serializes as `{"label", "memory_mib"}`. A label cannot contain `,` or `=` or
carry surrounding whitespace, so every ladder renders to text that parses back
to the same ladder. `SubstrateSizeClasses` is a non-empty
ladder sorted by memory with no two classes sharing a size; it serializes as an
array and parses from `small=2048,medium=8192,large=32768`. Construction,
parsing, and deserialization enforce both rules and fail with
`SubstrateSizeClassError`. The types do not select a class or schedule work.
