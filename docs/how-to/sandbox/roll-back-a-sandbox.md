# Roll back a running sandbox to a snapshot

Reset a running lease to one of your snapshots without changing its lease id, tenant or deadline. Processes and files in the sandbox are replaced by the snapshot's contents.

## Prerequisites

- A provider whose profile advertises `snapshot.supports_in_place_rollback`. Today that is the Docker worker and the Kubernetes worker. Other providers answer `unsupported`.
- A snapshot in your tenant's own snapshot repository, taken from a lease on a provider that can restore it.
- A lease in the `running` state. Resume a paused lease first.

## Roll back

With the Rust client:

```rust
let response = client
    .rollback_lease(tenant, &lease_id, snapshot_ref, None)
    .await?;
```

The request goes to `sandbox.lease.rollback.<lease_id>`. A snapshot from another tenant's repository is refused with `invalid_spec` before any worker is asked.

Pass an operation id as the last argument to make the call safe to retry. A repeat with the same id replays the stored result, and `get_operation` reads it after a timeout. The broker re-drives a rollback interrupted by a restart, and the Docker worker rebuilds again from the same snapshot, which gives the same result.

## What the worker does

1. Pulls and checks the snapshot. A bad snapshot fails here and the sandbox is left as it was.
2. Removes the old container.
3. Starts a new one from the snapshot with the request the lease was created with, under the same lease id.

If step 3 fails, the old container is already gone. The denial says so, and the lease should be stopped or rolled back again.

## Know the limits

- Live checkpoint artifacts are not accepted. Use a rootfs snapshot.
- The container id behind the lease changes, so the sandbox id stored in the lease record is the original one. Workers find the container by lease id.
- By default the Docker worker keeps the original lease request in memory. After a worker restart the lease is gone from the worker, and rollback answers not found.
- To survive a restart, start the Docker worker with `SANDBOX_DOCKER_PERSIST_ROLLBACK_STATE=true`. Each lease's request is then written under `SANDBOX_DOCKER_STATE_DIR` and removed when the lease is cancelled. The request can carry env values, so the files are mode 0600 in a 0700 directory. This is why the setting is off by default. Entries not rewritten for `SANDBOX_DOCKER_ROLLBACK_STATE_RETENTION_SECS` (default 604800, seven days) are removed at startup.
- The Kubernetes worker also rolls back in place and keeps the lease id. It verifies and pulls the snapshot first, so a bad snapshot leaves the sandbox as it was. It then deletes the lease's Job (or claimed warm pod), waits until the Job and its pods are gone, because a relaunch reuses the same Job name, and launches a new sandbox from the lease's request with the snapshot as the image. The original image becomes the snapshot's parent. A failure after the old sandbox is removed keeps the lease on the worker so the rollback can be retried or the lease cancelled. One rollback per lease runs at a time.
- On Kubernetes, to survive a worker restart, start the worker with `SANDBOX_K8S_PERSIST_ROLLBACK_STATE=true` (off by default). The request is then stored in a Secret named `<job>-lease-req`, owned by the Job or warm pod, so cluster garbage collection and the lease teardown remove it with the lease. The request can carry env values, which is why it is a Secret and why the setting is opt-in. Without it, a lease recovered after a restart has no stored request and rollback is denied with a message saying so. Turning the setting on affects leases created from then on. Older leases stay without a stored request.

## See also

- [Snapshot a running sandbox](./snapshot-a-running-sandbox.md) to take the snapshot.
- [Restore from a snapshot](./restore-from-snapshot.md) to start a new lease from one.
