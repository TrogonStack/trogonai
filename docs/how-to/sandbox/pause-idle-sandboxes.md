# Pause or release a sandbox when it goes idle

Have the broker act on a lease that has gone quiet: release it after a timeout, or pause it first and release it later.

## Release after a timeout

Set `spec.idle_timeout_seconds` when you create the lease. Once the broker has seen no activity (run, poll, kill, stdin, file read or write, or a worker activity signal) for longer than that, it releases the lease with reason `idle_timeout`. This works for every provider.

## Pause first, release later

Set `spec.idle_policy` instead. It is a list of stages, each measured from the last activity:

```json
{
  "idle_policy": {
    "stages": [
      { "after_seconds": 300, "action": "pause" },
      { "after_seconds": 3600, "action": "release" }
    ]
  }
}
```

The broker rejects the lease request unless:

- every `after_seconds` is between 1 and 86400, and they strictly increase;
- every action is strictly more severe than the one before (`pause`, then `suspend`, then `release`; no repeats and no milder action after a stronger one);
- `idle_timeout_seconds` is not also set. `idle_timeout_seconds: N` means the single stage `release` at `N`, so set one or the other.

From the TypeScript client, pass `idlePolicy` to `Sandbox.create`, with times in milliseconds (rounded up to whole seconds):

```ts
const sandbox = await Sandbox.create(client, {
  tenant,
  idlePolicy: [
    { afterMs: 5 * 60_000, action: 'pause' },
    { afterMs: 60 * 60_000, action: 'release' },
  ],
});
```

A `no_ttl` lease needs an idle limit; a `release` stage counts as one, so it can use `idle_policy` instead of `idle_timeout_seconds`.

## What each stage does

- **pause** goes through the same path as the `pause_lease` verb: the run deadline is parked, the paused budget (one hour by default) becomes the deadline, and paused time is tracked in usage. The lease is marked as paused by the idle policy. A provider whose capability profile does not support pause skips this stage, and the next stage fires instead.
- **suspend** goes through the same path as the `suspend_lease` verb with level `auto`, so the lease frees its host resources and keeps its id. The suspended budget (24 hours by default) becomes the deadline. If the provider cannot suspend the lease (it offers no suspend level, or the lease was created with snapshots off) the stage degrades to **pause**; if the provider cannot pause either, the stage is skipped. A suspend the worker refuses is retried with the same back-off as a failed idle pause.
- **release** releases the lease with reason `idle_timeout`.

Each stage fires once. Activity restarts the chain: resuming a paused lease restarts the idle clock and clears the idle-pause mark.

## How a paused lease meets the release stage

A lease the idle policy paused stays on the chain, so the `release` stage still fires at its time, measured from the last activity (pausing does not count as activity). The paused budget keeps applying too, so whichever comes first ends the lease: a budget that ends first releases it as `expired`. A lease you paused yourself with `pause_lease` is never touched by the idle policy; only its paused budget ends it. A policy with only a `pause` stage never releases the lease by idleness. A suspended lease leaves the chain: its suspended budget reaps it, so a `release` stage after a `suspend` stage does not fire for a lease that is already suspended.

## Suspend a sandbox and wake it later

Pause keeps the sandbox's resources allocated. Suspend releases them while the lease keeps its id and state, so it costs less for long gaps. Call the `suspend_lease` verb (`sandbox.lease.suspend.<lease_id>`, `sandbox_suspend_lease` over the WebSocket gateway) and wake the lease with `wake_lease` (`sandbox.lease.wake.<lease_id>`, `sandbox_wake_lease`).

```ts
const { level, reapAt } = await sandbox.suspend({ level: 'auto', maxSuspendedMs: 3_600_000 });
// later
await sandbox.wake();
```

`level` is `auto` (the default), `memory` or `disk`:

- **memory** saves memory and processes, so they continue after wake.
- **disk** saves only the disk; processes end.
- **auto** takes whatever the provider offers.

`memory` and `disk` must match the provider's `dormancy.suspend` capability exactly; there is no fallback to the other level. A provider that offers no suspend, or a different level, refuses the request with `suspend_unsupported` and the lease is left as it was. See the [provider option matrix](../../reference/sandbox/provider-option-matrix.md) for the level each worker offers. A lease created with snapshots off is refused with `snapshots_disabled`.

### Lease states and time

A lease moves `running` or `paused`, then `suspending`, then `suspended`; a wake goes `suspended`, `waking`, `running`. The run time left at suspend is parked and restored on wake, and the idle clock restarts. The suspended time budget (`max_suspended_seconds`, 24 hours by default, 7 days at most) becomes the deadline, so the sweeper releases a lease nobody wakes. Usage reports the time as `suspended_seconds`, and `suspend_artifact_bytes` when the provider names a stored artifact.

Both verbs are idempotent: suspending a suspended lease and waking a running lease succeed with no change. They accept an `operation_id` like pause and resume, and a suspend a crash interrupted is finished by the broker's retry of the stored operation.

### Errors

- `suspend_unsupported`: the provider cannot suspend to that level. Permanent.
- `lease_waking`: another request is waking the lease. Transient; the error carries `retry_after_ms` (the TypeScript client exposes it as `retryAfterMs`).
- `suspend_artifact_lost`: the saved state is gone, so the lease cannot wake and is failed. Permanent.

A wake that the worker fails returns the lease to `suspended`; after three failed wakes the lease is failed.

### Wake on access

A verb that needs a running lease (run command, file read and write, terminal, SSH access, domain lookup, user and group changes, native handle, snapshot) and meets a `suspended` lease starts the wake itself and answers `lease_waking` with `retry_after_ms`, the same answer a second caller gets while a wake is in flight. Retry after that delay. With the operation store the wake is a durable operation named after the lease, its suspend generation and its failed wakes so far, so concurrent callers join one wake and a broker restart finishes it; without the store it runs as a detached task guarded by the same compare-and-swap into `waking`. Verbs that only read or edit lease metadata (tags, timeout, network policy), poll or kill a command, and pause or resume still answer `lease_not_running` for a suspended lease. A woken lease restarts its idle clock, so the idle chain does not suspend it again at once.

Suspended leases still count against the active lease cap like paused ones.
