# Snapshot store (ORAS)

This page explains how the platform uses an OCI-compliant registry as a content-addressed snapshot store for derived sandbox artifacts: what an OCI artifact is, how `Digest` pinning works, how snapshots flow from lease release through push into NATS KV, and why the registry is a trust root. Platform engineers, broker authors and worker implementers should read it before touching snapshot push or restore code.

## OCI artifacts are not container images

A container image is one kind of OCI manifest: layers plus a config blob that describes a root filesystem and entrypoint. An OCI artifact is any other manifest whose top-level `artifactType` (or, on older registries, `config.mediaType`) declares a different purpose. The sandbox stack stores warm-start filesystem snapshots, Hyperlight WASM snapshots, Firecracker snapshots, KubeVirt disk images, SBOMs and scan reports as artifacts, not as runnable container images.

Each artifact manifest has:

- An `artifactType` string, mapped to `ArtifactType` in `trogon-sandbox-contract`.
- One or more content layers (the actual bytes: tar, bincode, qcow2, JSON).
- An optional `subject` field pointing at another manifest (the base image, a parent snapshot, or the artifact being attested).

The workers and the snapshot builder talk to the registry through the `trogon-sandbox-oci` crate (`rsworkspace/crates/sandbox/trogon-sandbox-oci`), not by shelling out to a CLI on the hot path. Operators can still use the `oras` CLI to inspect the registry.

## Why an OCI registry

A snapshot is a large, immutable, content-addressed blob with a few attached attestations. An OCI registry already provides exactly that: blobs and manifests addressed by digest, a `subject` field and the referrers listing for signatures, SBOMs and scan reports, standard authentication, and tooling operators already run. The platform adds a typed vocabulary (`ArtifactType`) on top and keeps the registry a single platform-owned store, separate from the read-only base image catalog and from mutable volumes.

## What each worker captures

| Worker | `ArtifactType` it advertises | Payload |
|---|---|---|
| `trogon-sandbox-docker-worker` | `SandboxRootfsV1` | The container root filesystem as a gzipped tar layer |
| `trogon-sandbox-k8s-worker` | `SandboxRootfsV1` | A root filesystem tar |
| `trogon-sandbox-kubevirt-worker` | `KubevirtDiskV1` | The VM disk as qcow2 |
| `trogon-sandbox-hyperlight-worker` | `HyperlightSnapshotV1` | The serialized Hyperlight snapshot |
| `trogon-sandbox-firecracker-worker` | `FirecrackerSnapshotV1` | A Firecracker snapshot (see [Execution environment identity](#execution-environment-identity-env-digest)) |

Workers that advertise no artifact type do not take part in the registry store: for example the subprocess, Daytona, Proxmox and Agent Substrate workers report `can_snapshot` and `can_restore` as unsupported. Run `trogon-sandbox-admin capability show` to read a worker's profile.

## Content addressing with `Digest`

Every blob and manifest in an OCI registry is identified by a SHA-256 digest. In `trogon-sandbox-contract`, `Digest` is a validated newtype: the string must start with `sha256:` followed by 64 lowercase hex digits.

Lookups use the digest, not a tag. Tags exist only for human navigation. The broker, workers and KV records always pin an exact `Digest`. A worker refuses to mount a snapshot unless the pulled manifest digest matches the broker-recorded value in JetStream KV (bucket `sandbox-broker-kv-snapshot`, logical path `sandbox.broker.kv.snapshot.{digest}`).

On push, the client computes the manifest digest from the layers and annotations and refuses to push when it differs from the `OciArtifactRef` the caller expects (`OciError::DigestMismatch`). On pull, layers are verified against their descriptors, and a bounded pull enforces limits on layer count, per-layer size and total size before any layer is downloaded. Registry metadata responses are size-limited while streaming (manifests at 1 MiB, token responses at 64 KiB, tag pages at 4 MiB), and blob downloads drop credentials if a redirect leaves the original origin.

This immutability is the integrity primitive. The OCI client verifies digests on every pull, and Cosign signatures attach as referrers and are verified at pull time.

## `ArtifactType` and Trogon media types

`ArtifactType` is the typed enum for artifacts the platform owns. Each variant maps to a canonical `application/vnd.trogon.*` media type via `ArtifactType::media_type()`.

| `ArtifactType` variant | Media type |
|---|---|
| `SandboxRootfsV1` | `application/vnd.trogon.sandbox.rootfs.v1+tar` |
| `HyperlightSnapshotV1` | `application/vnd.trogon.hyperlight.snapshot.v1` |
| `FirecrackerSnapshotV1` | `application/vnd.trogon.firecracker.snapshot.v1` |
| `KubevirtDiskV1` | `application/vnd.trogon.kubevirt.disk.v1+qcow2` |
| `SbomV1` | `application/vnd.trogon.sbom.v1` |
| `ScanReportV1` | `application/vnd.trogon.scan-report.v1` |

Primary snapshot payloads use the first four. `SbomV1` and `ScanReportV1` are attached as referrers (through the OCI `subject` field) to a primary snapshot manifest, and are not lease restore inputs on their own.

Wire references use `OciArtifactRef`, which carries a registry, a repository, a `Digest` and an `ArtifactType`. Leases select a snapshot through `SandboxImage::Snapshot { reference, parent }`, where `reference` is an `OciArtifactRef` and the optional `parent` is an `OciImageRef` for the base image the snapshot was derived from. The full media-type contract is in [Media types](../../reference/sandbox/media-types.md).

## Referrers API v1.1 and the tag-scheme fallback

OCI Distribution Spec v1.1 adds a Referrers API endpoint, `GET /v2/<name>/referrers/<digest>`. It lists manifests that declare the target digest in their `subject` field (signatures, SBOMs, scan reports).

Some registries do not advertise this endpoint. The spec defines a fallback tag scheme, where referrers are discoverable through synthetic tags of the form `sha256-<digest>`. The OCI crate handles both paths and counts fallbacks in the metric `trogon_sandbox_oci_referrers_fallback_total`. Prefer a registry with native Referrers API support so discovery stays cheap. Verify that your registry supports it before depending on recursive referrer discovery.

## Snapshot flow: build, push, record, restore

The control plane is NATS only.

```
Lease with snapshot_out
        |
        v
  Worker runs sandbox
        |
        v
  LeaseReleased (NATS)
        |
        v
  Snapshot builder (reacts to released leases where snapshot_out is set)
        |
        +--> NATS request: worker export (docker commit, Hyperlight serialize, ...)
        |
        +--> OCI push to the platform registry
        |
        +--> optional: attach SBOM, scan report and Cosign signature as referrers
        |
        v
  Write the digest record to JetStream KV (sandbox-broker-kv-snapshot)
        |
        v
  Publish SnapshotPublished on sandbox.snapshot.published
```

On a subsequent lease:

1. The caller issues a `LeaseRequest` with `SandboxImage::Snapshot { reference, ... }`.
2. The broker selects a worker whose `ProviderCapabilityProfile` satisfies the restore requirements (see [Provider capability profiles](./capability-profiles.md)) and, ideally, whose `WorkerHeartbeat.cached_digests` already contains the digest.
3. The worker pulls the artifact by digest, verifies it against KV, and restores it (Hyperlight decodes its snapshot, Docker imports the rootfs tar, and so on).
4. A worker-side digest cache reduces cold-pull latency on repeat restores.

Repository layout in the platform-owned registry is `snapshots/<tenant>`, or `snapshots/tenant-hex/<hex of the tenant id>` for tenant ids that contain `:` or `/`. See [KV key layout](./kv-key-layout.md) for why. The digest in NATS KV, not the tag, is authoritative.

## Tags, retention and garbage collection

Digests are authoritative and tags are navigation only. Every snapshot push leaves a staging tag, and the catalog keeps only the newest N snapshots per lease, with an optional sweep for orphaned artifacts. The tag format, the keep-N rule, the sweep and its environment variables are described in [Snapshot catalog, keep-N retention and the orphan sweep](./snapshot-catalog.md). Registry-side garbage collection consults the live digest set the broker maintains in KV. Disaster recovery and promotion between registries are covered in [Back up snapshots](../../how-to/sandbox/snapshot-backup.md).

Two costs follow from the artifact sizes. Root filesystem snapshots can reach hundreds of megabytes or more, so retention limits are the main control on registry growth. A first restore of a digest on a worker pulls the artifact on the critical path; the `cached_digests` list in the worker heartbeat lets the broker prefer a worker that already holds the digest.

## Snapshot store as trust root

The snapshot registry is a trust root. Any identity that can push arbitrary artifacts can poison another tenant's warm start. The platform enforces these boundaries.

| Actor | Push | Pull |
|---|---|---|
| Snapshot builder | Yes (platform credentials) | Yes |
| Workers | No (export only, the builder pushes) | Yes (scoped, digest-pinned) |
| Tenants and clients | No | Indirect (a lease that references a known digest) |

- **Digest pinning.** Workers refuse snapshots not recorded in broker KV. This closes the gap where a registry tag is repointed to a different manifest.
- **Cosign signing.** With `SANDBOX_COSIGN_KMS_URI` set, the builder signs each published snapshot with Cosign, and the signature attaches as a referrer. Workers that verify signatures do so before restore. Without the setting, the builder skips signing.
- **Scan gate.** With `SANDBOX_REQUIRE_SCAN_PASS` enabled (the default), the builder scans the artifact before publish. A snapshot that fails the scan is not published.
- **Pull secrets for base images.** `SandboxImage::Registry { pull_secret: Option<PullSecretId> }` references credentials held by the credential broker, never inlined on the wire. See [Add a pull secret](../../how-to/sandbox/add-a-pull-secret.md).
- **Per-tenant isolation.** Registry access control limits tenants to pull-only against their own repository. Pushes use a single platform identity. See [Tenant-scoped snapshot registry RBAC](./registry-rbac.md).
- **Garbage collection coordination.** The broker maintains the live digest set in KV so registry-side retention consults a known live set. The Zot chart in `devops/helm/charts/zot` reaps unreferenced manifests: every retention policy it renders sets `deleteUntagged` to true, and `gc.delay` and `gc.interval` control how old an untagged manifest must be and how often the sweep runs. A registry other than Zot needs the equivalent setting, such as a Harbor project retention policy.

## Execution environment identity (`env_digest`)

A Firecracker memory snapshot only resumes correctly under the same VMM, kernel, boot arguments and CPU shape that produced it. The worker therefore hashes that environment and treats the result as part of the snapshot's identity.

**The digest.** `EnvironmentDefinition` (in `trogon-sandbox-contract`) is the canonical description: provider, architecture, a sorted component map, boot arguments, snapshot format version, isolation profile and CPU template. Its digest is the sha256 of a fixed domain prefix plus the compact JSON of that struct, rendered `sha256:<hex>`. The Firecracker worker fills the components at start from the VMM version, the VMM binary digest, the kernel digest and, when the jailer is used, the jailer binary digest. Changing any input changes the digest, and the same inputs always give the same digest.

**Where it is recorded.**

- `SnapshotMetadata.env_digest` inside the artifact. It is omitted when absent, so old metadata still parses.
- The manifest annotation `io.trogon.sandbox.env-digest`. The snapshot builder copies it into the Cosign signed payload, so the signature covers it. A malformed value is refused instead of signed.
- The worker capability profile (`placement.env_digest`).

**Restore refusal.** Before any file is written, the worker compares the snapshot's recorded digest with its own. A different digest is refused. When the manifest annotation and the metadata inside the artifact disagree, the restore is refused too.

**Snapshots without a digest.** Snapshots taken before the field existed are allowed with a warning by default. `--strict-snapshot-env` (`SANDBOX_FIRECRACKER_STRICT_SNAPSHOT_ENV`) refuses them. Turn it on once old snapshots have aged out.

**Node execution policy.** `--approved-env-digests` (`SANDBOX_FIRECRACKER_APPROVED_ENV_DIGESTS`, comma separated) lists the digests this node may run. The worker refuses to start when its own digest is not in the list. An empty list approves only the worker's own digest. `--cpu-template` feeds the CPU template into both the machine config and the digest.

**Placement.** `CapabilityRequirements.env_digest` restricts a lease to workers advertising the same digest, in both the capability check and the broker's always-on gate. The broker derives the digest itself: the snapshot builder records the manifest's `io.trogon.sandbox.env-digest` in the snapshot's catalog record, and a restore by digest or by tag overwrites whatever the request put in `required_capabilities.env_digest` with that value. A snapshot the catalog does not know, or one with no digest, leaves the request's own value in place, and the worker still checks the snapshot against its own environment.

**Signature verification on restore.** The Firecracker worker verifies the Cosign signature of a snapshot it pulls from the registry against a configured public key, and requires the signed payload to carry the same environment digest the snapshot records. A registry that relabels a snapshot, or serves one whose signature does not cover the digest, fails the check.

- `--cosign-public-key` (`SANDBOX_COSIGN_PUBLIC_KEY`): path to a PEM public key.
- `--cosign-verify` (`SANDBOX_COSIGN_VERIFY`): `require` (default), `warn` or `off`. `warn` and `off` apply only when set explicitly. `warn` logs a failed or impossible verification and restores anyway. `require` refuses the restore (a signature error is reported as `SnapshotSignatureInvalid`), and the worker refuses to start without a key.

Rollout: the worker refuses to start with the default `require` until a public key is configured. To roll the key out gradually, set `warn` explicitly, configure the public key on every worker, watch the logs until no restore warns, then remove the override so `require` applies. Snapshots signed before the digest annotation existed verify on signature alone and are still governed by `--strict-snapshot-env`. Local (held) snapshots are not pulled from the registry and are not signature checked.

## Three storage layers

The reference architecture splits storage into three layers:

1. Base image catalog (multi-registry): read-only container images through `SandboxImage::Registry`.
2. Snapshot store (this page): derived, content-addressed artifacts.
3. Volumes (S3): mutable workspace data. See [Volumes](./volumes.md).

Restoring a snapshot does not restore a volume. They mount side by side.

## Provider-native snapshots

Some providers snapshot natively and hand back an opaque id, not an OCI artifact (for example Modal, E2B and, with a workspace volume, Fly). That id travels as a `NativeSnapshotRef` with no digest and no registry location. It is a separate path from the registry store, not a mirror of it.

- **Taking one.** A live checkpoint request carries `native_name`. A worker whose profile sets `supports_native_snapshots` snapshots under that name and replies `SnapshotExportOutcome::Native`. The broker picks the name, so it can record ownership before the snapshot exists. A provider that assigns its own id, as Modal does, ignores the name and returns that id, and the broker records the receipt's reference. A snapshot lease returns `CreatedNative` with a `NativeSnapshotReceipt`, but only to a caller that sets `accept_native`. Any other caller gets `Unsupported`.
- **Restoring one.** The client passes `SandboxImage::NativeSnapshot { reference }`. The broker checks that the id is well formed, not expired, listed in the request's provider preference and recorded for the requesting tenant, then limits placement to workers of that provider that advertise `supports_native_snapshots` and `can_restore`. Unknown, foreign-tenant, expired and wrong-provider references all fail as `InvalidSpec`, and a foreign tenant's reference reads the same as an unknown one.
- **Ownership.** The broker records each reference per tenant in the `sandbox-native-snapshots` KV bucket. The provider-side name is the only secret the reference holds, so the record, not the name, is the authority.
- **Fork.** Fork of a native-only provider checkpoints natively and restores from the reference.

Modal's reference is the id of a `modal.Image` from `Sandbox.snapshot_filesystem`. It captures the filesystem only, not memory, and Modal retains it for 30 days, which the receipt reports as `expires_at`. A restore builds the sandbox from that image.

Fly's reference is a volume snapshot, so it exists only when the worker runs with a per-lease workspace volume. It captures the workspace files, not memory.

Native references are not portable: only a worker of the same provider can restore one, and the provider owns retention. `expires_at` is set only when the provider reports a lifetime.

### Mixed versions

Every addition is optional on the wire and defaults to off.

| Peer is older | What happens |
|---|---|
| Client | Never sets `accept_native`, so a native-only provider answers `Unsupported`. It never sees `created_native`. |
| Broker | Does not know the `native_snapshot` image kind and rejects the request at parse time. A client that only restores native references needs a new broker. |
| Worker | Ignores `native_name`. A worker that does not advertise `supports_native_snapshots` is never sent a native restore, because the broker filters on the flag. |
| Profile | A profile without `supports_native_snapshots` reads as unsupported. |

## Limitations

- Only workers whose capability profile lists the artifact type can restore it. The subprocess, Daytona, Proxmox and Agent Substrate workers advertise no snapshot support, so a lease that requires a snapshot is not placed on them. Use a worker from the table above, or check `trogon-sandbox-admin capability show`.
- The `cached_digests` locality hint is reported by the Kubernetes worker. The Modal and Azure Container Apps workers send an empty list, so placement for those workers does not account for warm digests.
- Native snapshots capture the filesystem, not memory, and are not portable to another provider. Use a registry snapshot when the artifact must move between workers.
- Referrer discovery depends on the registry. A registry without the referrers endpoint is served through the tag fallback, which is slower and counted in `trogon_sandbox_oci_referrers_fallback_total`.

## See also

- [Volumes](./volumes.md): snapshots versus volumes.
- [Provider capability profiles](./capability-profiles.md): worker selection.
- [Publish a snapshot](../../how-to/sandbox/publish-a-snapshot.md): post-run capture.
- [Restore from a snapshot](../../how-to/sandbox/restore-from-snapshot.md): lease from digest.
- [Back up snapshots](../../how-to/sandbox/snapshot-backup.md): air-gapped disaster recovery.
- [Media types](../../reference/sandbox/media-types.md): media type contract.
