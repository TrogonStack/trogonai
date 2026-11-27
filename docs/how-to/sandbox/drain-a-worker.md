# Drain or cordon a worker

Take a worker out of placement before you restart, patch or replace it. The
broker stops choosing that worker for new leases. Leases it already holds are
not touched.

## Prerequisites

- `trogon-sandbox-admin` pointed at the same NATS as the broker. Set
  `NATS_URL` or pass `--nats-url`.
- The worker's provider token (`docker`, `k8s` and so on) and its worker id, as
  shown in its heartbeat.

## Cordon a worker

```sh
trogon-sandbox-admin worker cordon --provider docker --worker docker-1 --reason "kernel patch"
```

The state is written to the `sandbox-worker-fences` KV bucket. Every broker
re-reads it every 5 seconds, so allow about five seconds before you rely on it.
A broker that cannot read the bucket keeps the last state it saw, so a KV outage
does not put a cordoned worker back into rotation.

## Drain a worker

```sh
trogon-sandbox-admin worker drain --provider docker --worker docker-1
```

Drain cordons the worker and then lists the live leases the broker attributes
to it. Run `worker status` again until the list is empty, then stop the worker.

```sh
trogon-sandbox-admin worker status --provider docker --worker docker-1
```

Drain does not move leases. A lease ends when its owner stops it or its
deadline passes. Migrating live leases to another worker is not automated.

## Put the worker back

```sh
trogon-sandbox-admin worker uncordon --provider docker --worker docker-1
trogon-sandbox-admin worker list
```

`list` shows every cordoned or draining worker.

## Know the limits

- A fence is keyed by provider and worker id. A replacement that reuses the id
  inherits the fence, so uncordon it when it is ready.
- Operations on a lease the worker already holds, such as pause, resume and
  snapshot, still reach it, because they are routed by lease and not placed.
- `--active-lease-bucket` (`SANDBOX_ACTIVE_LEASE_BUCKET`, default
  `sandbox-active-leases`) must match the broker's bucket for `drain` and
  `status` to count leases correctly.
