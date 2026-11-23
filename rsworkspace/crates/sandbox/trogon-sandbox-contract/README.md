# Sandbox contract

`trogon-sandbox-contract` provides validated worker, dispatch-attempt, and operation
identifiers, artifact metadata, operation kinds, and the operation state transition
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
