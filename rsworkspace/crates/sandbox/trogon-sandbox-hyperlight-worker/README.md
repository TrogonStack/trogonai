# trogon-sandbox-hyperlight-worker

Serves sandbox leases as Wasm components in hypervisor-isolated guests.

The worker queue-subscribes to Hyperlight lease dispatches, loads the component or snapshot a lease
names into a guest, and keeps that guest until the lease is cancelled. A cancelled guest goes back to a
bounded warm pool. Snapshots travel as OCI artifacts of type `HyperlightSnapshotV1`.

## The hypervisor seam

Everything that touches a hypervisor sits behind the `Hypervisor` trait. The lease rules, the warm pool,
the snapshot transport and the authority guard are plain code that runs and is tested without one.

The shipped binary uses `UnsupportedHypervisor`. It exits at startup with a clear error on a host that
cannot run guests, instead of advertising capacity it does not have. A deployment that has a hypervisor
backend passes it to `serve` or `launch`.

## Configuration

| Flag | Environment | Default |
| --- | --- | --- |
| `--component-root` | `SANDBOX_HYPERLIGHT_COMPONENT_ROOT` | required |
| `--nats-url` | `SANDBOX_NATS_URL` | `nats://127.0.0.1:4222` |
| `--max-concurrent-leases` | `SANDBOX_MAX_CONCURRENT_LEASES` | `8` |
| `--max-warm-sandboxes` | `SANDBOX_MAX_WARM_SANDBOXES` | `4` |
| `--snapshot-registry` | `SANDBOX_SNAPSHOT_REGISTRY` | `127.0.0.1:5000` |
| `--insecure-registries` | `SANDBOX_OCI_INSECURE_REGISTRIES` | none |
| `--dispatch-fence-file` | `SANDBOX_DISPATCH_FENCE_FILE` | in memory |

The image of a lease is a logical name that resolves to a `.wasm` file under the component root. Names
that are absolute or contain `..` are refused.

## What a lease may ask for

A guest is a sealed Wasm component, so a lease is refused when it asks for `spec.env`, a clone, host
mount or S3 workspace, privileged access, or a provider-native snapshot. A snapshot image must be a
`HyperlightSnapshotV1` artifact, and the broker resolves snapshot tags before dispatch.
