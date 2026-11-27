# What a sandbox keeps, and how to keep less

Every lease has two retention switches, `retention.monitoring` and `retention.snapshots`, both on by default. This page lists what the stack retains, with the control for each, and states where no control exists.

## What is retained and the control that limits it

| Data | Where | Control per sandbox |
| --- | --- | --- |
| Disk state after the lease ends | Snapshot store, when a snapshot is captured | `retention.snapshots: false` refuses every capture. Without it, leave `snapshot_out` unset and set `persistent: false` on a named lease. |
| Command stdout and stderr | Worker memory while the command runs and until it is polled | `output_retention: stream_only` keeps no stdout and only a 64 KiB stderr tail. The stream delivers the output instead. |
| Command history | The broker's command log stream, so `read_logs` can replay output | `retention.monitoring: false` purges the lease's history when it ends. Without it, the stream's size and age limits age it out. |
| Lease usage counters | Broker KV bucket, keyed by lease id | None per sandbox. Counters hold numbers only (commands run, bytes moved, seconds), never command text or file names. Entries expire seven days after they were last written. |
| Lease record and tags | Broker active lease store, while the lease is live | Not retained after the lease ends beyond the usage entry above. Do not put sensitive values in tags, which listings can read. |
| Cancellation fences and cleanup ownership receipts | Broker operations and provider custody stores | Retained while their identity namespace accepts requests. The output and history switches do not remove the evidence needed to refuse replay or reconcile resources. See [cleanup receipt retention](../../reference/sandbox/cleanup-receipt-retention.md). |

A name is claimed atomically when a create starts. The claim is pending for the broker's provider dispatch timeout and becomes confirmed once the lease is recorded, so a crashed request never blocks the name for longer than that timeout. A second request that arrives while the first is still pending is denied with `name_conflict`, which is transient: retry with the same idempotency key. For a named create the lease id is derived from the tenant, the name and the idempotency key, so retrying with the same key returns the lease the first attempt provisioned instead of conflicting with it or provisioning twice.

A named lease is persistent by default, so naming a sandbox turns on disk capture unless you pass `persistent: false`. In the TypeScript client (see [sandbox foundation](../sandbox-foundation.md)):

```ts
import { Sandbox, SandboxWsClient, tenantId } from '@trogonai/sandbox-ws-client';

const client = await SandboxWsClient.connect({
  gatewayHttpUrl: process.env.SANDBOX_GATEWAY_HTTP_URL!,
  gatewayWsUrl: process.env.SANDBOX_GATEWAY_WS_URL!,
  apiKey: process.env.SANDBOX_API_KEY!,
});

const sandbox = await Sandbox.create(client, {
  tenant: tenantId('acme'),
  providerPref: ['local_docker'],
  name: 'scratch-run',
  persistent: false,
});
await sandbox.delete();
client.close();
```

## Per-lease toggles

Both toggles live on the spec as `retention`. The broker enforces them, so they hold on every provider without a worker change.

- `snapshots: false` denies `snapshot_lease` and `fork` with `snapshots_disabled`, refuses a spec that also sets `snapshot_out`, and turns off the automatic capture a named persistent lease would get. The name still reconnects while the lease runs. Pause is unaffected: it freezes the sandbox in place and captures nothing.
- `monitoring: false` keeps live streaming and `read_logs` working while the lease runs, then purges its command history from the log stream on release, expiry or idle timeout. A worker may still be flushing a command's last chunks at that moment, so the purge runs a second time 30 seconds later to remove any that landed after it. A purge failure is logged and the stream's own limits still apply. Usage counters are kept, because they hold numbers only.

The toggles cover what this stack keeps. A provider's own logs or checkpoints on its side of the provider API are outside the broker's reach. The worker-level `sandbox.worker.<id>.snapshot.export` subject is internal to the stack and driven only by `snapshot_out`, which the broker refuses for these leases. Keep it out of client NATS permissions. The worker does not know a lease's toggles, so it cannot refuse an export itself, and NATS cannot tell a legitimate exporter from another holder of worker credentials. Give each worker credential subscribe permission only on its own `sandbox.worker.<id>.snapshot.export`, and give publish permission on that subject only to the snapshot builder and broker credentials.

In the TypeScript client the toggles are `monitoring` and `snapshots` on `Sandbox.create`. The command line client takes `--no-monitoring` and `--no-snapshots` on the lease command:

```ts
const sandbox = await Sandbox.create(client, {
  tenant,
  providerPref: ['local_docker'],
  monitoring: false,
  snapshots: false,
});
```

## Limits of the controls

- There is no per-sandbox opt-out for the usage counters. They hold numbers only, so none is needed today.
- A provider that captures state outside this stack's snapshot store, such as a hosted provider's own checkpoints, is outside these toggles.

## Related

- [Snapshot store](./snapshot-store.md)
- [Capability profiles](./capability-profiles.md)
