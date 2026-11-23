# Sandbox contract

`trogon-sandbox-contract` provides validated worker, dispatch-attempt, and operation
identifiers, lease identifiers and policies, artifact metadata, operation kinds, and
the operation state transition rules.

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

`LeaseId`, `TenantId`, and `CorrelationId` serialize as JSON strings and accept
any string on deserialization, so identifiers minted before a rule existed keep
parsing. `TenantId::new` validates a tenant being introduced: it rejects an empty
value, one longer than 128 bytes, or one containing a control character.
`EmptyObject` serializes as `{}` for enum variants that carry no fields, and
deserializes only from an empty JSON object.

`SandboxTags` serializes as a JSON object of string values and rejects more than
five tags on construction and deserialization. `contains_all` matches a tag
filter, and an empty filter matches every lease.

`IdlePolicy` lists `IdleStage` values, each applying an `IdleAction` (`pause`,
`suspend`, or `release`) once a lease has been quiet for more than
`after_seconds`. `IdlePolicy::validate` requires at least one stage, stage times
within the operator's limit and strictly increasing, and strictly more severe
actions.

`SnapshotPolicy` names the `ArtifactType` and target repository for a lease's
snapshots, plus an optional `RetentionPolicy`, `PrewarmSpec`, and tag name that
are omitted from JSON when absent. `RetentionPolicy::new` and `validate` keep
`max_tags` between 1 and 10; deserialization does not validate, so stored
policies keep parsing. `delete_evicted` is an `EvictedArtifactAction`, `Delete`
or `Keep`, serialized as a JSON boolean; an omitted value reads as `Keep`, which
leaves the evicted artifact in the registry. These policies describe intent;
they do not schedule idle handling, take snapshots, or prune a registry.

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

This package defines values and allowed transitions. It does not execute
operations, persist state, authorize transitions, enforce claim ownership, or
provide worker, provider, transport, or lease-generation behavior.
