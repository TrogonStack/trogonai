# Deploy the sandbox stack with Helm

This guide installs the sandbox control plane from the umbrella chart at `devops/helm/charts/trogon-sandbox`: NATS JetStream, the Zot snapshot store, SpiceDB, the broker, the snapshot builder, the credential broker, the gateways, the dashboard and the workers. It also covers the operator tasks that come after the install.

## Prerequisites

- A Kubernetes cluster with a default `StorageClass` for the JetStream and backup volumes.
- Helm 3.
- Container images for every enabled `trogon-sandbox-*` binary. The chart names each image after the binary it runs, for example `<imageRegistry>/trogon-sandbox-broker`, and pulls it from `global.imageRegistry` at `global.imageTag`. Build and push the images to the registry that value points at.
- Optional: a KMS key id for the credential broker (`global.kmsKeyId`).

## Install

From the repository root:

```bash
helm lint devops/helm/charts/trogon-sandbox
helm template sandbox ./devops/helm/charts/trogon-sandbox --debug > /tmp/sandbox-render.yaml

kubectl create namespace sandbox --dry-run=client -o yaml | kubectl apply -f -

helm install sandbox ./devops/helm/charts/trogon-sandbox \
  --namespace sandbox \
  --set global.imageTag=<your-tag>
```

The chart runs the broker with authorization enforced (`trogon-sandbox-broker.insecureDev: false`): bearer authentication, the image allow-list and the worker registry are on, and the broker reads `SPICEDB_ENDPOINT` and `SPICEDB_TOKEN`. The `spicedb` subchart deploys SpiceDB and writes the schema at every start.

### Create the Secrets first

The chart does not hold secret values. Create these Secrets before installing, or the pods do not start.

The SpiceDB preshared key, under the name and key `global.spicedb.presharedKeySecret` points at (default `sandbox-spicedb` and `preshared-key`). Both SpiceDB and the broker read it from there:

```bash
kubectl -n sandbox create secret generic sandbox-spicedb \
  --from-literal=preshared-key="$(openssl rand -hex 32)"
```

The dispatch signing material, under `global.dispatchAuth.secretName` (default `sandbox-dispatch-auth`) with the keys `signing-key` and `public-keys`. The chart sets `global.dispatchAuth.verify` to `require`, so every worker refuses an unsigned dispatch. See [Sign lease dispatches and verify them on workers](./sign-worker-dispatches.md) for the key format and a staged rollout through `warn`.

### SpiceDB choices

- The default datastore is `memory`, which loses every relationship when the pod restarts. For anything beyond evaluation set `spicedb.datastore.engine=postgres` and `spicedb.datastore.uriSecret.name` to a Secret holding the connection URI.
- Grant access with `spicedb.bootstrapRelationships`, for example `sandbox_tenant:acme#admin@service_account:ci-runner`, or write relationships to SpiceDB yourself.
- To use an external SpiceDB set `spicedb.enabled=false` and `global.spicedb.endpoint=host:port`.
- Before upgrading an existing deployment, follow [Migrate sandbox identities](./migrate-sandbox-identities.md).
- For a local stack without authorization install with `-f devops/helm/charts/trogon-sandbox/values.dev.yaml`, which sets `trogon-sandbox-broker.insecureDev=true` and skips SpiceDB.

The `mise run github:workflows:ci-sandbox-deploy-checks:check-charts` task fails if the default render turns authorization off or lacks `SPICEDB_TOKEN`.

### In-cluster endpoints

Release name `sandbox` yields these endpoints:

| Service | DNS |
|---------|-----|
| NATS | `sandbox-nats:4222` |
| Zot registry | `sandbox-zot:5000` |
| Snapshot builder metrics | `sandbox-sandbox-snapshot-builder:9095` |
| WebSocket gateway | `sandbox-sandbox-ws-gateway:8443` |
| Dashboard | `sandbox-sandbox-dashboard:8080` |

Override the URLs when using another release name:

```bash
helm install mysandbox ./devops/helm/charts/trogon-sandbox \
  --set global.natsUrl=nats://mysandbox-nats:4222 \
  --set global.registryUrl=mysandbox-zot:5000
```

### Enable optional workers

The Docker, KubeVirt, Daytona, ACA, Modal and subprocess workers are disabled by default because they need a host socket or cloud credentials. Enable them per provider:

```bash
helm upgrade sandbox ./devops/helm/charts/trogon-sandbox \
  --reuse-values \
  --set trogon-sandbox-docker-worker.enabled=true \
  --set trogon-sandbox-kubevirt-worker.enabled=true
```

The subprocess worker chart exposes the egress proxy options under `egressProxy` (`bindingTtlSeconds`, `revalidateIntervalSeconds`, `resweepLiveFlows`, `requireTerminationForNameOnlyAllows`, `tlsExemptHosts`, `tlsExemptAddresses`). All are off by default. See [Egress: name to address attribution](../../architecture/sandbox/egress-dns-binding.md#configuring-the-daemon).

## Run more than one Kubernetes worker replica

A Kubernetes worker recovers lease Jobs and claimed warm pods only when their worker identity matches its configured identity and Kubernetes confirms their namespace and UID. Resources without this ownership proof stay untouched.

The chart runs a single-replica Deployment by default and refuses `replicaCount` above 1 with it. To scale out, run the worker as a StatefulSet:

```yaml
trogon-sandbox-k8s-worker:
  workloadKind: StatefulSet
  replicaCount: 3
  recoverUnlabeledLeases: false
```

Each pod gets its own name, such as `<release>-sandbox-k8s-worker-0`, as `SANDBOX_K8S_WORKER_IDENTITY` through the downward API. Keep the identity stable across restarts and unique among running workers. The same identity appears on resource labels, lease grants, heartbeats and authority queries. An unset identity generates a new value at startup, so a later process cannot automatically recover the earlier process's resources.

Before upgrading an installation with unlabeled resources, inventory its live leases and reconcile each resource with its actual owner. Do not relabel or delete a resource because its name resembles a lease. When scaling down, drain the removed ordinal or preserve its identity for later recovery; other replicas leave its leases untouched. Set `recoverUnlabeledLeases` to `false` once no unlabeled leases remain.

`orphanReclaimGraceSecs` (`SANDBOX_K8S_ORPHAN_RECLAIM_GRACE_SECS`) sets how long a no-deadline lease's owning replica must stay absent before another replica reclaims the lease. Unset uses the worker default; `0` turns the reclaim off.

By default an agent whose lease names no user runs as uid 65532, whatever the image's `USER`, so images that default to root start. Set `honorImageUser: true` (`SANDBOX_K8S_HONOR_IMAGE_USER`) to keep the image's own non-root `USER`. An image whose `USER` is root, unset or not a numeric id falls back to uid 65532 with a warning, so the agent never runs as root. A lease that names a user always gets that user.

### Size the warm pool

The Kubernetes worker keeps warm pods labelled `sandbox.trogon.live/warm-pool`, so a restarted worker counts them from the cluster instead of starting from empty. It also paces refill, so a restart or a burst of claims does not create a pod flood:

| Flag | Environment variable | Default | Meaning |
| --- | --- | --- | --- |
| `--warm-pool-refill-per-pass` | `SANDBOX_WARM_POOL_REFILL_PER_PASS` | `2` | Most warm pods one reconcile pass may create |
| `--warm-pool-max-warming` | `SANDBOX_WARM_POOL_MAX_WARMING` | `4` | Most warm pods allowed to be starting at once |

A failed warm claim is not a lease failure: the lease cold starts instead.

The KubeVirt worker sizes its warm pool from these variables:

| Variable | Meaning |
| --- | --- |
| `SANDBOX_KUBEVIRT_WARM_POOL_ENABLED` | `false` cold starts every lease and keeps no idle VMI |
| `SANDBOX_KUBEVIRT_WARM_POOL_MAX_PER_DIGEST` | Idle warm VMIs kept per snapshot digest |
| `SANDBOX_KUBEVIRT_WARM_POOL_MAX_TOTAL` | Warm VMIs kept across every digest |
| `SANDBOX_KUBEVIRT_WARM_POOL_MAX_IDLE_SECS` | Seconds an idle warm VMI lives before it is deleted |
| `SANDBOX_KUBEVIRT_WARM_MEMORY_MIB` | Guest memory of every warm VMI; leases asking for more cold start |

## Preserve KubeVirt lease ownership during rollout

Set `workerId` (`SANDBOX_WORKER_ID`) to a unique identity for every concurrently running KubeVirt worker. Empty uses the release-scoped worker name. Lease grants, heartbeats, resource ownership labels and authority queries use this identity.

Keep the state directory (`stateDir`, `SANDBOX_KUBEVIRT_STATE_DIR`) on persistent storage across worker replacements. The directory is private to one worker identity and guest namespace, with an exclusive process lock. It stores resource receipts and pending allocation intents, never SSH keys, credentials or lease environment values. A storage error prevents admission; there is no in-memory fallback.

The chart requires an existing persistent volume claim and uses a single replica with the `Recreate` strategy:

```yaml
trogon-sandbox-kubevirt-worker:
  enabled: true
  workerId: sandbox-kubevirt-a
  image: <disk image a lease boots when it names none>
  persistence:
    existingClaim: sandbox-kubevirt-custody-a
```

The chart mounts the volume at `/var/lib/sandbox-kubevirt` and sets UID, GID and filesystem group 65532. Use storage that supports the directory lock, atomic replacement and durable file synchronization. Separate worker deployments need distinct identities and custody volumes.

Startup recovers receipts before it starts handlers, CREATE intake or heartbeats. A previously ready lease returns to service only after its original VM, disk and owned VMI are verified. Unknown cold-create outcomes or unresolved ownership stop startup for explicit reconciliation. Keep the original journal and resources while investigating; names or copied labels alone cannot authorize adoption or deletion.

Drain active leases before upgrading a worker that predates durable custody, because its process-local receipts cannot be rebuilt from an empty journal. Preserve an existing journal when replacing a worker or restoring storage.

## Fence credential requests with committed policies

The chart values turn the committed-policy fence on: `trogon-sandbox-broker` `commitCredentialPolicies`, `trogon-sandbox-subprocess-worker` `requireCommittedPolicy` and `trogon-sandbox-credential-broker` `fenceMode: required`. The broker needs the `nats-kv` quota backend, which the chart uses by default, to write the shared bucket.

On an existing install, upgrade in order: broker first, workers second, credential broker last. Set the worker value to `false` and the credential broker value to `optional` until leases created before the upgrade have ended. A worker that runs outside this chart, such as the Firecracker worker, needs `SANDBOX_REQUIRE_COMMITTED_POLICY=true` in its environment.

## Verify the install

1. Check NATS health:

   ```bash
   kubectl port-forward svc/sandbox-nats 8222:8222 -n sandbox
   curl -sf http://127.0.0.1:8222/healthz
   ```

2. Check the snapshot builder metrics:

   ```bash
   kubectl port-forward deploy/sandbox-sandbox-snapshot-builder 9095:9095 -n sandbox
   curl -sf http://127.0.0.1:9095/metrics | head
   ```

3. Check the Zot registry:

   ```bash
   kubectl run curl-zot --rm -i --restart=Never -n sandbox \
     --image=curlimages/curl:8.5.0 -- curl -sf http://sandbox-zot:5000/v2/
   ```

4. Mint an API key with the admin CLI:

   ```bash
   kubectl exec -it deploy/sandbox-sandbox-broker -n sandbox -- \
     trogon-sandbox-admin api-key mint --tenant acme --service-account ci-runner --name smoke
   ```

   If `trogon-sandbox-admin` is not in the broker image, run it from a debug pod with `NATS_URL` pointing at `nats://sandbox-nats:4222`.

5. Open the dashboard:

   ```bash
   kubectl port-forward svc/sandbox-sandbox-dashboard 8080:8080 -n sandbox
   ```

   Then browse to `http://127.0.0.1:8080`.

Most control-plane binaries are NATS-only and expose no `/healthz`. The chart uses exec probes (`<binary> --help`) or TCP and metrics probes where HTTP exists, so a passing probe does not prove readiness. For metric ports and alerts see [Scrape sandbox metrics and export traces](./observability.md) and [Alerts](../../reference/sandbox/alerts.md).

## Recover snapshot tags after a builder restart

The snapshot builder creates the JetStream stream and durable consumer named `sandbox-snapshot-tag-recovery` before it accepts lease releases. The consumer first requests durable accounting confirmation from a broker on `sandbox.snapshot.account.published`, then retries the tag update with its original release time and retention policy. A refused, malformed, mismatched, unanswered or timed-out confirmation keeps the queued obligation. Recovery never re-exports the snapshot or republishes an event that is already queued.

Builder replicas share the `sandbox-snapshot-builders` release queue group and the same durable consumer, with at most 64 unacknowledged events.

With the NATS CLI and a reachable `NATS_URL`, inspect pending work:

```bash
nats --server "$NATS_URL" stream info sandbox-snapshot-tag-recovery
nats --server "$NATS_URL" consumer info sandbox-snapshot-tag-recovery sandbox-snapshot-tag-recovery
```

Keep the stream and consumer when restarting or replacing a builder. Purging the stream discards pending accounting and tag work; deleting the durable consumer resets its delivery state. For a growing backlog, restore broker and tag KV availability and inspect the builder logs for accounting, compare-and-swap or acknowledgement failures. See [Snapshot catalog](../../architecture/sandbox/snapshot-catalog.md) and [Snapshot store](../../architecture/sandbox/snapshot-store.md) for how the catalog and store fit together.

## Recover snapshot accounting and interrupted deletion

Preserve the `sandbox-snapshot-accounting-v1` and `sandbox-snapshot-live-v1` KV buckets when replacing a broker. Each tenant's nonexpiring accounting row holds its host capture reservations, bound OCI receipts, live identities, stored count and byte caps, and deletion receipts. Metadata and live projections use tenant-scoped digest keys, so identical content owned by different tenants stays independent.

- An export with no confirmed receipt remains an unresolved capture. A generic transient worker result or RPC timeout does not prove that the export had no effect, so automatic replay stops with an unknown outcome. Only a NATS `NoResponders` result proves that dispatch did not occur and can be retried automatically.
- Do not guess a digest or discard a reservation to unblock deletion.
- Unreceipted legacy usage, missing generations and legacy shared repositories need explicit reconciliation. Keep their stored usage and caps; do not zero them.
- Upgrade brokers and builders together, then reconcile legacy ownership before admitting new accounted publications for an affected tenant.
- Registry deletion requires an exact durable host claim. Only a positive registry deletion response completes the claim and refunds the stored bytes. A timeout, interrupted process or uncertain response leaves the claim pending and keeps its charge.

Before reconciling an interrupted claim, settle or fence the original executor and its outstanding registry request. Observing that the digest is absent does not prove that an earlier DELETE has finished, and reopening capture could let that request delete newly uploaded identical content. Reconcile the recorded claim, receipt and physical artifact together, and do not remove the accounting row or its fence as a shortcut.

## Reconcile a legacy CREATE whose reply was lost

The broker stores a durable dispatch intent before it sends a legacy CREATE. If the provider reply is lost or unusable, recovery keeps the operation unsettled and preserves its quota reservation. It does not resend the request to another worker replica.

Before retrying unresolved work, locate the original worker allocation and reconcile it with the operation's tenant, lease ID, attempt ID and original quota reservation. An allocation name alone is insufficient. Do not clear the dispatch intent, refund its quota or submit a fresh operation to bypass the unresolved state while the original allocation may still be running.

## Scope worker NATS users for the lease authority check

A worker asks the broker which of its leases are still assigned to it with a request on `sandbox.worker.authority.<worker_id>`. Workers carry no credential the broker can verify, so the broker only checks that the worker id in the subject equals the one in the body. Scope every worker's NATS user to publish only on its own `sandbox.worker.authority.<worker_id>` subject, never on the `sandbox.worker.authority.>` wildcard, so one worker cannot query another's leases. See [Client NATS permissions](../../reference/sandbox/client-nats-permissions.md) for the permission model.

Set `SANDBOX_WORKER_AUTHORITY` to one of:

- `off`: no authority check.
- `warn` (the default): log and count only. An older broker that does not serve the verb produces one warning and no change to any lease.
- `enforce`: freeze, then purge, leases the broker does not assign to the worker. An unavailable authority service starts partition fencing after 45 seconds, and the guard purges suspect leases after a further 60 second grace period.

Before enabling `enforce`, reconcile or end nonterminal legacy leases whose broker record has no `worker_id`. An unbound record confirms no worker's ownership. Keep affected workers in `warn` until the actual holder is verified and its assignment is recorded, or its resources are released. Do not infer an owner from whichever worker asks first.

## Network policies

When `global.networkPolicy.enabled=true` (the default):

- NATS accepts client traffic only from pods labeled `sandbox.trogonai.io/stack=true`.
- Worker egress follows the `sandbox.trogonai.io/egress-enforcement` label (`mandatory` or `best-effort`). This bounds the worker pod, not the guest sandbox, so a worker whose control plane is a remote HTTPS API needs `best-effort` however strict its guest isolation is.
- The KubeVirt chart's `guestEgressBackstop` (off by default) restricts guest VMs to DNS and NATS in the guest namespace, which is useful because that worker advertises no egress enforcement.
- KubeVirt guests are NATed behind their virt-launcher pod, so the pod network sees the pod and not the guest. Whether the CNI enforces NetworkPolicy between pods remains the cluster's responsibility. See [Isolation runtimes](../../architecture/sandbox/isolation-runtimes.md#guest-network-anti-spoofing-and-residual-risk).

## Garbage collection

Zot garbage collection is enabled by default in the Zot subchart (`devops/helm/charts/zot/values.yaml`):

| Value | Default | Zot field |
|-------|---------|-----------|
| `gc.enabled` | `true` | `storage.gc` |
| `gc.delay` | `2h` | `storage.gcDelay` (untagged manifests older than this) |
| `gc.interval` | `1h` | `storage.gcInterval` (sweep cadence) |

Retention policies (`retention.defaultPolicy`, `retention.perTenant`) map to `storage.retention.policies`. The default keeps 90 days and the last 50 tags per repository. See [Data retention](../../architecture/sandbox/data-retention.md).

The broker maintains a live digest set in the `sandbox-broker-kv-snapshot` KV bucket for garbage collection to consult. Do not delete tagged snapshots that still appear in active leases.

## Replicate across regions

Single-region installs leave `replication.enabled: false`, the default. For multi-region mirroring set `replication.enabled: true` and populate `replication.syncRegistries` with upstream URLs, credentials and content prefixes. Zot renders these under `extensions.sync`.

Sync needs credentials in both directions: the downstream cluster must authenticate to each upstream registry, and the workers that trigger on-demand sync need upstream pull credentials. Read [Registry RBAC](../../architecture/sandbox/registry-rbac.md) before granting cross-region pull access.

## Back up and run a recovery drill

Set `backup.repositories` to the snapshot repositories to protect, one per tenant (`snapshots/<tenant id>`). The chart then renders a `<release>-snapshot-backup` CronJob, on `backup.schedule` (default `0 3 * * *`), and its PVC. With an empty list neither renders.

The drill script tears down Zot, restores from the latest backup and verifies that a snapshot is queryable. It requires `TEST_REPO` to name one of the repositories in `backup.repositories`:

```bash
RELEASE=sandbox NAMESPACE=sandbox TEST_REPO=snapshots/acme \
  mise run sandbox:dr-drill
```

## Uninstall

```bash
helm uninstall sandbox -n sandbox
kubectl delete namespace sandbox-leases --ignore-not-found
# PVCs (JetStream, backup) are retained unless explicitly deleted.
kubectl delete pvc -n sandbox -l app.kubernetes.io/part-of=sandbox-stack
```

## See also

- [Registry RBAC](../../architecture/sandbox/registry-rbac.md)
- [Alerts](../../reference/sandbox/alerts.md)
- Chart values: `devops/helm/charts/trogon-sandbox/values.yaml`
