# Snapshot a running sandbox and start another from it

A snapshot of a running lease is either an OCI artifact in the snapshot registry or a provider-native capture. Keep its receipt to start a new sandbox from it on a provider that can restore that format.

## Steps

1. Snapshot the lease without stopping it.

   ```ts
   const receipt = await sandbox.snapshot();
   ```

   The receipt carries a `reference` and the size in `bytes`. Over MCP and the gateway the verb is `sandbox_snapshot_lease`. In Rust it is `SandboxClient::snapshot_lease`.

2. Start a new sandbox from it.

   ```ts
   const copy = await Sandbox.fromSnapshot(client, receipt, { tenant });
   ```

   This is `Sandbox.create` with `fromSnapshot`, which sets `spec.image = { kind: "snapshot", reference }`. Combining `fromSnapshot` with `image` is an `invalid_request`.

3. Delete the snapshot when it is no longer needed.

   ```ts
   const existed = await Sandbox.deleteSnapshot(client, receipt.reference.digest);
   ```

   A repeat resolves `false`. Over MCP and the gateway the verb is `sandbox_snapshot_delete`.

## Provider-native snapshots

Opt in to provider-native receipts when capturing a running sandbox:

```ts
const receipt = await sandbox.snapshot({ acceptNative: true });
const copy = await Sandbox.fromSnapshot(client, receipt, { tenant });
if ('provider' in receipt.reference) {
  await Sandbox.deleteNativeSnapshot(client, receipt.reference);
}
```

The native reference belongs to the authenticated tenant. Retire it with `client.nativeSnapshotDelete(reference)`, the gateway tool `sandbox_native_snapshot_delete`, or Rust's `SandboxClient::native_snapshot_delete`. The broker request subject is `sandbox.native_snapshot.delete`. Native deletion is implemented for E2B. Other providers return `unsupported`. OCI deletion continues to use a digest.

Deletion intent prevents new restores, including when a provider failure requires a retry. A restore already admitted may finish in its own sandbox. Deleting the capture does not delete that sandbox or its source. An expired reference can still be retired. A completed repeat returns `not_found`, which the `Sandbox` convenience method maps to `false`.

E2B refuses template retirement while a running fork uses it. The delete call reports a backend refusal and the cleanup intent remains durable. Release that fork when its work is finished, then retry deletion. Bounded recovery can also settle the pending retirement after the dependency has gone.

## Pause and resume

`sandbox.pause({ maxPausedMs })` and `sandbox.resume()` freeze and thaw a sandbox in place, keeping its id and state. See [Pause or release a sandbox when it goes idle](./pause-idle-sandboxes.md).

## When a worker cannot do it

| Situation | Typed answer |
|---|---|
| The lease's provider cannot checkpoint a running lease | `sandbox_snapshot_lease` resolves `outcome: "unsupported"`. The TypeScript `sandbox.snapshot()` throws `unsupported`. |
| No worker can restore the snapshot reference | The lease is denied with `no_worker_can_restore_snapshot`. |
| The provider cannot pause | Pause and resume resolve `outcome: "unsupported"`. The TypeScript wrappers throw `unsupported`. |
| The broker has no snapshot registry access | `sandbox_snapshot_delete` resolves `outcome: "unsupported"`. |
| The lease is not found or not owned by the caller | Denied with `lease_not_found`. |

## Worker support

Taken from each worker's `ProviderCapabilityProfile`.

| Worker | Live snapshot (`can_checkpoint_running`) | Start from a snapshot (`can_restore`) | Pause and resume |
|---|---|---|---|
| docker | yes | yes | yes |
| firecracker | yes, when enabled by config | yes, when enabled by config | no |
| kubevirt | yes | yes | no |
| hyperlight | yes | yes | no |
| k8s | yes | yes | no |
| memory | no | no | yes |
| e2b | native memory | native memory | yes |
| modal | native filesystem | native filesystem | no |
| daytona | no | no | yes |

For other workers and configuration-dependent support, use the [provider option matrix](../../reference/sandbox/provider-option-matrix.md) and the worker's guide. Native capture support does not imply native deletion support.

The TypeScript memory fake models snapshots in process, so tests can cover the whole flow without a registry. The Rust memory worker replies with a typed unsupported answer, which the conformance suite checks.

## See also

- [Roll back a running sandbox to a snapshot](./roll-back-a-sandbox.md) to reset a lease in place.
- [Restore from a snapshot](./restore-from-snapshot.md).
- [Snapshot store](../../architecture/sandbox/snapshot-store.md).
