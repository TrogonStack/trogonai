# Use native passthrough and region placement

This guide explains how to pin a sandbox to a region and how to pass a vendor
option the portable `LeaseRequest` does not model, then read back the vendor's
own sandbox id. It is for callers who need one specific provider behavior and
for operators who decide which tenants may ask for it.

## Pick a region

Set `region` on the lease. Add `fallback_regions` to name the regions to try, in
order, when none of the workers serving `region` can take the lease.

```json
{
  "region": "eu",
  "fallback_regions": ["us"]
}
```

The broker keeps only workers whose heartbeat profile lists the region in
`placement.regions`, then runs its usual capability and capacity selection on
what is left. The first region with an eligible worker wins and the worker
receives that region as its `region`.

The broker enforces these rules:

- `fallback_regions` without `region` is refused as an invalid request.
- Region names must be non-empty.
- When no worker serves any listed region, the lease fails with
  `no_worker_in_region`, which names the regions asked for. It is transient,
  because workers join and leave, so retry later or add a fallback.
- When the real obstacle is something else, such as no worker at all or no
  worker with the needed capability, that error is returned instead of
  `no_worker_in_region`.
- A worker that lists no regions is never chosen for a lease that names one.

Workers declare regions in their capability profile:

| Worker | Regions |
|--------|---------|
| Fly | The one configured region |
| Modal | `TROGON_SANDBOX_MODAL_REGIONS`, comma separated |
| E2B | `SANDBOX_E2B_REGIONS`, comma separated |
| Vercel | `SANDBOX_VERCEL_REGIONS`, comma separated |
| Other workers | None |

A worker sends the region it receives to its provider. The Daytona worker sends
it as the Daytona `target`.

## Pass vendor options

`native` is an object keyed by provider kind. Only the entry for the provider
that gets the lease reaches its worker. Every other entry is dropped before
dispatch.

```json
{
  "provider_pref": ["daytona"],
  "native": {
    "daytona": { "auto_stop_interval": 30 }
  }
}
```

Three checks apply, in this order:

1. Tenant policy. Native passthrough is denied by default. A tenant that is not
   allowed gets `native_passthrough_not_permitted`.
2. Shape. `native` must be an object keyed by known provider kinds, and a
   request that sets it only goes to workers whose profile declares
   `placement.supports_native_passthrough`.
3. Worker allowlist. The worker accepts a fixed list of keys and refuses
   anything else, naming the key, as `invalid_native_options`. Nothing is
   created before this check.

### Allowed keys

| Provider | Key | Type and range |
|----------|-----|----------------|
| `daytona` | `auto_stop_interval` | minutes, 0 to 525600 |
| `daytona` | `auto_archive_interval` | minutes, 0 to 525600 |
| `daytona` | `auto_delete_interval` | minutes, 0 to 525600 |
| `daytona` | `public` | boolean |
| `local_docker` | `pids_limit` | 1 to 1048576 |
| `local_docker` | `shm_size_mib` | 1 to 65536 |
| `e2b` | `auto_pause` | see [Run the E2B worker](./e2b-worker.md) |
| `vercel` | `persistent` | boolean, see [Run the Vercel Sandbox worker](./vercel-worker.md) |

A Docker lease with native options skips the warm pool, because a pooled
container was started with the worker's own limits.

## Allow a tenant

Store a per-tenant record with `trogon-sandbox-admin`. The broker reads it on
every check, so no restart is needed.

```sh
trogon-sandbox-admin native-passthrough set --tenant acme
trogon-sandbox-admin native-passthrough show --tenant acme
trogon-sandbox-admin native-passthrough set --tenant acme --deny
trogon-sandbox-admin native-passthrough clear --tenant acme
```

Records live in the NATS KV bucket `sandbox-native-passthrough`. Point the
broker at another bucket with `--native-passthrough-bucket` or
`SANDBOX_NATIVE_PASSTHROUGH_BUCKET`. Keys follow the tenant key layout in
[KV key layout](../../architecture/sandbox/kv-key-layout.md).

You can also list tenants when starting the broker:

```sh
trogon-sandbox-broker --native-passthrough-tenants acme,globex
```

The environment variable is `SANDBOX_NATIVE_PASSTHROUGH_TENANTS`, also comma
separated.

The broker decides in this order:

1. A tenant on the static list is allowed, whatever its stored record says.
2. Otherwise a stored record decides: allowed with `set`, denied with
   `set --deny`.
3. A tenant with neither is denied.

With `--quota-backend in-memory` the stored records live in broker memory and
`trogon-sandbox-admin` cannot reach them, so use the static list there.

## Read the vendor handle

After the lease is running, ask for the handle. It is read-only and needs the
same tenant permission.

| Surface | Call |
|---------|------|
| Rust client | `client.native_handle(&lease_id)` |
| Gateway tool | `sandbox_native_handle` with `{ "lease_id": "..." }` |
| MCP tool | `sandbox_native_handle` |
| TypeScript | `sandbox.nativeHandle()` or `client.getNativeHandle(leaseId)` |

The reply is `available` with `provider`, `vendor_id` and, when the vendor has
one, `url` and `region`, or `unsupported` when the provider has no such handle.
For Daytona, `vendor_id` is the sandbox id. For the TypeScript client, see
[Sandbox foundation](../../architecture/sandbox-foundation.md).

## Errors

| Error kind | Meaning | Retry |
|------------|---------|-------|
| `native_passthrough_not_permitted` | The tenant is not allowed. | No |
| `invalid_native_options` | The worker refused a key or value. The reason names it. | No |
| `no_worker_in_region` | No worker serves any requested region. | Yes, later |

## Related

- [Capability profiles](../../architecture/sandbox/capability-profiles.md) for
  `PlacementCapability`
- [Provider option matrix](../../reference/sandbox/provider-option-matrix.md)
- [Run the Daytona worker](./daytona-worker.md)
