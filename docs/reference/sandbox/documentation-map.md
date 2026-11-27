# Sandbox documentation map

Every sandbox page, grouped by what it is for. Tutorials teach by doing, how-to guides solve one task, explanations say why the system works the way it does, and references are lookup tables.

The TypeScript client for the sandbox broker is covered separately in [the sandbox client foundation](../../architecture/sandbox-foundation.md).

## Tutorials

| Page | Use it to |
|---|---|
| [Your first sandbox lease](../../get-started/sandbox/first-sandbox-lease.md) | Start the broker and subprocess worker locally and lease a sandbox from Rust |
| [Onboard a new tenant end-to-end](../../get-started/sandbox/onboard-tenant.md) | Give a tenant its own bucket, key, API keys and quotas |
| [Publish a signed, scanned snapshot](../../get-started/sandbox/publish-signed-snapshot.md) | Push a sandbox snapshot to the registry with a signature and scan results |

## How-to guides

### Run and operate the stack

| Page | Use it to |
|---|---|
| [Run the whole sandbox stack locally with one command](../../how-to/sandbox/run-the-stack-locally.md) | Start NATS, the broker, a worker and the WebSocket gateway as one process group with `trogon-sandbox-dev` |
| [Run the subprocess worker on macOS](../../how-to/sandbox/run-the-subprocess-worker-on-macos.md) | Run leases under Seatbelt on a Mac |
| [Deploy the sandbox stack with Helm](../../how-to/sandbox/deploy.md) | Install the broker, workers and registry on Kubernetes |
| [Run the sandbox Docker smoke test](../../how-to/sandbox/docker-smoke.md) | Check a Docker-backed stack end to end |
| [Validate the sandbox stack on a Linux KVM host](../../how-to/sandbox/validate-on-a-linux-kvm-host.md) | Run the Firecracker worker checks on a host with `/dev/kvm` |
| [Drain or cordon a worker](../../how-to/sandbox/drain-a-worker.md) | Take a worker out of rotation without dropping leases |
| [Scrape sandbox metrics and export traces](../../how-to/sandbox/observability.md) | Wire the stack into Prometheus and OpenTelemetry |
| [Sign lease dispatches and verify them on workers](../../how-to/sandbox/sign-worker-dispatches.md) | Make workers reject dispatches the broker did not sign |
| [Migrate sandbox identities](../../how-to/sandbox/migrate-sandbox-identities.md) | Move an existing installation to the sandbox-owned bucket, tenant type and worker resource names |
| [Change a public wire type safely](../../how-to/sandbox/guard-the-wire-contract.md) | Evolve contract types without breaking running peers |

### Run a specific worker

| Page | Use it to |
|---|---|
| [Run the Firecracker worker](../../how-to/sandbox/firecracker-worker.md) | Run leases in Firecracker microVMs |
| [Run the Daytona worker](../../how-to/sandbox/daytona-worker.md) | Run leases on Daytona |
| [Run the E2B worker](../../how-to/sandbox/e2b-worker.md) | Run leases on E2B |
| [Run the Modal worker](../../how-to/sandbox/modal-worker.md) | Run leases on Modal |
| [Run the SPACE worker](../../how-to/sandbox/space-worker.md) | Run leases on SPACE |
| [Run the V8 and Wasm worker](../../how-to/sandbox/v8-wasm-worker.md) | Run JavaScript and Wasm guests in process |
| [Run the Vercel Sandbox worker](../../how-to/sandbox/vercel-worker.md) | Run leases on Vercel Sandbox |
| [Run a sandbox on Agent Substrate v1](../../how-to/sandbox/run-a-substrate-sandbox.md) | Run leases through the Agent Substrate provider |
| [Use native passthrough and region placement](../../how-to/sandbox/native-passthrough-and-regions.md) | Pass provider-specific options and pin leases to regions |

### Images, storage and access

| Page | Use it to |
|---|---|
| [Build and use the default agent image](../../how-to/sandbox/build-an-agent-image.md) | Give coding workloads an image with Git and a compiler toolchain |
| [Add a registry pull secret](../../how-to/sandbox/add-a-pull-secret.md) | Lease images from a private registry |
| [Mount remote storage into a sandbox](../../how-to/sandbox/mount-remote-storage.md) | Attach S3 prefixes as streaming FUSE mounts |
| [Expose a sandbox service through a signed preview](../../how-to/sandbox/expose-sandbox-ports.md) | Reach a port inside a sandbox from outside |
| [Open an interactive terminal](../../how-to/sandbox/open-an-interactive-terminal.md) | Attach a shell to a running sandbox |
| [Stream live stdout and stderr from a running command](../../how-to/sandbox/stream-command-output.md) | Follow command output as it is produced |
| [Run a headless browser in a sandbox](../../how-to/sandbox/run-a-headless-browser.md) | Drive a browser from an agent |
| [Run multiple agents as separate Linux users in one sandbox](../../how-to/sandbox/run-multi-agent-users.md) | Separate agents that share a sandbox |
| [Pause or release a sandbox when it goes idle](../../how-to/sandbox/pause-idle-sandboxes.md) | Stop paying for sandboxes nobody uses |

### Snapshots

| Page | Use it to |
|---|---|
| [Publish a snapshot](../../how-to/sandbox/publish-a-snapshot.md) | Push a lease's workspace to the registry |
| [Restore from a snapshot](../../how-to/sandbox/restore-from-snapshot.md) | Start a lease from a published snapshot or a named tag |
| [Snapshot a running sandbox and start another from it](../../how-to/sandbox/snapshot-a-running-sandbox.md) | Fork a live sandbox |
| [Roll back a running sandbox to a snapshot](../../how-to/sandbox/roll-back-a-sandbox.md) | Return a sandbox to an earlier state |
| [Back up and restore the snapshot registry](../../how-to/sandbox/snapshot-backup.md) | Protect the registry against data loss |

## Explanation

| Page | Explains |
|---|---|
| [Isolation tiers and runtimes](../../architecture/sandbox/isolation-runtimes.md) | Which workers isolate leases how, and how to choose one |
| [Subprocess isolation](../../architecture/sandbox/subprocess-isolation.md) | What bubblewrap and Seatbelt confine for the subprocess worker |
| [Provider capability profiles](../../architecture/sandbox/capability-profiles.md) | How workers declare what they support and how the broker routes on it |
| [Exec semantics across workers](../../architecture/sandbox/exec-semantics.md) | How commands run and report results on each worker |
| [Egress: name to address attribution and TLS interception exemptions](../../architecture/sandbox/egress-dns-binding.md) | How egress rules bind host names to addresses |
| [Volumes](../../architecture/sandbox/volumes.md) | How mutable workspace storage works and how it differs from snapshots |
| [Snapshot store (ORAS)](../../architecture/sandbox/snapshot-store.md) | How snapshots are stored as OCI artifacts |
| [Snapshot catalog, keep-N retention and the orphan sweep](../../architecture/sandbox/snapshot-catalog.md) | How snapshots are indexed and pruned |
| [Tenant-scoped snapshot registry RBAC](../../architecture/sandbox/registry-rbac.md) | How registry access is limited to each tenant |
| [KV key layout for tenant-scoped records](../../architecture/sandbox/kv-key-layout.md) | How broker records are keyed in JetStream |
| [Persistent sandbox entity: modes, soak plan and intent reconciler](../../architecture/sandbox/entity-rollout.md) | How the persistent entity store is rolled out and what the intent reconciler does |
| [What a sandbox keeps, and how to keep less](../../architecture/sandbox/data-retention.md) | What the stack retains for each lease, and the control for each item |
| [The Agent Substrate provider](../../architecture/sandbox/substrate-provider.md) | How the Agent Substrate provider fits behind the broker |

## Reference

| Page | Lists |
|---|---|
| [Provider option matrix](./provider-option-matrix.md) | Where each worker's declared capabilities live, and Docker hardening defaults |
| [Trogon OCI media types](./media-types.md) | Media types of snapshot artifacts and their layers |
| [NATS permissions for a client credential](./client-nats-permissions.md) | Subjects a client credential needs |
| [Credential rule scope and the resolution fence](./credential-rule-scope.md) | The fields that narrow which hosts receive a credential |
| [Cleanup authority and receipt retention](./cleanup-receipt-retention.md) | Who may clean up a lease and how long receipts are kept |
| [Sandbox Prometheus alert rules](./alerts.md) | Shipped alert rules and what they watch |
| [E2B runtime contract](./e2b-runtime.md) | How the E2B worker behaves and where its operator boundaries are |
| [Agent Substrate v1 adapter contract](./substrate-v1-contract.md) | The Agent Substrate v1 interface the adapter uses |
| [Sandbox testing policy](./testing-policy.md) | Rules for sandbox crate tests |
