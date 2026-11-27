# NATS permissions for a client credential

This reference lists the NATS subjects a client-facing credential needs publish or subscribe permission for, and the subjects it must never get. It applies to anything that talks to the broker over NATS directly (the `SandboxClient` in `rsworkspace/crates/sandbox/trogon-sandbox-client`, or `trogon-sandbox-ws-gateway` acting for browser and SDK sessions).

The source of truth for subject names is `rsworkspace/crates/sandbox/trogon-sandbox-protocol/src/subjects.rs` and `constants.rs`. The broker authorization code is under `rsworkspace/crates/sandbox/trogon-sandbox-broker/src/auth`.

## What the broker enforces

The broker does not scope NATS subjects to tenants. The lease id in a subject such as `sandbox.lease.run_command.<lease_id>` is not checked against the credential that published it.

What the broker does check is an API key. When broker auth is enforced (`--authz-enforce`, env `SANDBOX_AUTHZ_ENFORCE`, default on; `--insecure-dev` or `SANDBOX_INSECURE_DEV=true` turns it off for a local stack and logs a startup warning), the broker reads `Authorization: Bearer <key>` from the NATS message headers, looks the key up, requires the key's tenant to equal the tenant it is bound to, and asks the authorizer (SpiceDB). `SandboxClient::with_api_key` sets that header. The check runs on every client verb except the capability list:

- Verbs whose request carries a `tenant` are bound to that tenant: lease request and fork, usage, limits, delete, stop, pause, resume, snapshot, snapshot delete, drives, list, reap, get by name, snapshot get and tree, tags and named delete. Verbs that also carry a `lease_id` rely on the handler to reject a lease owned by another tenant.
- Verbs addressed by lease id alone are bound to the tenant that owns the lease, looked up by the broker: cancel, tags update, network policy update, extend timeout, run, poll, kill, stop and restart command, read logs, write stdin, list processes, wait ready, terminal, domain, native handle, SSH access, files, users and groups. An unknown lease answers `lease_not_found`.

A refused request gets `unauthorized` (`DriveError` for drives). Replies with no error variant (drive list, lease list, snapshot tree, tag history) come back empty, and get-by-name and snapshot get come back not found. Cancel has no reply, so a refused cancel is only logged. File transfers are checked after the payload is received, because the lease id is inside it.

Not checked: the capability list (the request has no tenant or lease, and the profiles are provider-level), and the subjects workers publish (heartbeats, lease activity, snapshot published and deleted events, evidence). Evidence, terminal output and release events are plain NATS subscriptions with no broker check at all. With `--insecure-dev` no verb is checked.

Treat NATS permissions as the isolation boundary between tenants. Give each tenant its own credential, and do not hand a credential to untrusted code unless every subject in it is acceptable for that code to use.

## Subjects a client needs

"Publish" means the client sends a request and reads the reply. Replies arrive on the client's own inbox, so every request and reply client also needs the inbox rule in the next section.

| Subject pattern | Direction | Answered or published by | Why a client needs it |
| --- | --- | --- | --- |
| `sandbox.lease.request` | publish | broker | Ask for a lease. |
| `sandbox.lease.fork` | publish | broker | Fork a running lease. |
| `sandbox.lease.get_by_name` | publish | broker | Look up a lease by tenant-scoped name. |
| `sandbox.lease.list` | publish | broker | List the tenant's leases. |
| `sandbox.lease.reap` | publish | broker | Release leases by tag, age or idleness. |
| `sandbox.lease.cancel.<lease_id>` | publish | broker | Release a lease. Sent with a reply subject, the broker answers `cancelled` or `denied`; a plain publish gets no reply. |
| `sandbox.lease.delete.<id>`, `stop.<id>`, `pause.<id>`, `resume.<id>` | publish | broker | Lifecycle verbs. |
| `sandbox.lease.suspend.<id>`, `wake.<id>` | publish | broker | Suspend a lease to memory or disk and wake it again. |
| `sandbox.lease.snapshot.<id>` | publish | broker | Snapshot a running lease. |
| `sandbox.lease.rollback.<id>` | publish | broker | Roll a running lease back to one of the tenant's snapshots. |
| `sandbox.lease.extend_timeout.<id>`, `tags.update.<id>`, `network_policy_update.<id>` | publish | broker | Change a running lease. |
| `sandbox.lease.usage.<id>`, `sandbox.tenant.usage`, `sandbox.tenant.limits` | publish | broker | Read usage and limits. |
| `sandbox.lease.run_command.<id>`, `poll_command.<id>`, `kill_command.<id>`, `stop_command.<id>`, `restart_command.<id>`, `cancel_command_launch.<id>`, `write_stdin.<id>`, `list_processes.<id>`, `wait_ready.<id>`, `wait_sandbox_ready.<id>` | publish | broker (forwards to the worker) | Run and manage commands. |
| `sandbox.lease.read_logs.<id>` | publish | broker (answers from the command log stream) | Read retained command output. |
| `sandbox.lease.write_files.<id>`, `read_file.<id>`, `file_op.<id>` | publish | broker (forwards to the worker) | File access. |
| `sandbox.lease.create_user.<id>`, `create_group.<id>`, `modify_group_membership.<id>` | publish | broker (forwards to the worker) | Sandbox users and groups. |
| `sandbox.lease.get_domain.<id>`, `native_handle.<id>`, `ssh_access.<id>` | publish | broker (forwards to the worker) | Resolve exposed ports, read the vendor handle, request SSH access. |
| `sandbox.lease.terminal.<id>` | publish | broker (forwards to the worker) | Terminal control: open, input, resize, close. |
| `sandbox.terminal.out.<terminal_id>` | subscribe | worker | Live terminal output. See below. |
| `sandbox.evidence.<lease_id>.>` | subscribe | worker | Per-lease evidence, including streamed command output on `command.<execution_id>`. |
| `sandbox.lease.released.<lease_id>` | subscribe | broker | Learn that a lease ended. |
| `sandbox.drive.create`, `list`, `delete` | publish | broker | Tenant drives. |
| `sandbox.snapshot.get`, `delete`, `tree` | publish | broker | Snapshot lookup, delete and lineage. |
| `sandbox.native_snapshot.delete` | publish | broker | Retire a tenant-owned provider-native snapshot. |
| `sandbox.snapshot.tag.set`, `tag.delete`, `tag.history` | publish | broker | Named snapshot tags. |
| `sandbox.named.delete` | publish | broker | Delete a sandbox by name when no lease holds it. |
| `sandbox.capability.list` | publish | broker | List provider capability profiles. |
| `sandbox.operation.capabilities` | publish | broker | Check whether the broker supports create and restore cancellation before dispatch. Tenant scoped. |
| `sandbox.operation.cancel` | publish | broker | Record create and restore cancellation intent; optionally request compensation of a verified newly-owned allocation. Tenant scoped. |
| `sandbox.operation.get` | publish | broker | Read a stored lifecycle outcome and optional compensation state by operation id. Tenant scoped. |
| `sandbox.entity.get`, `sandbox.entity.list`, `sandbox.entity.delete` | publish | broker | Read, list and delete persistent sandbox entities. Tenant scoped. Needs `SANDBOX_ENTITY_MODE`; see below. |

The entity verbs depend on the broker flag `SANDBOX_ENTITY_MODE` (or `--entity-mode`):

- `off`: no entity store exists. All three verbs answer a denial of kind `entity_not_enabled`, which is permanent.
- `shadow` (default): the broker mirrors named persistent sandboxes into the entity store without changing any response. `get` and `list` work; `delete` answers `entity_not_enabled`. The rollout plan to `on` is in [Entity rollout](../../architecture/sandbox/entity-rollout.md).
- `on`: the entity is the source of truth for named persistent sandboxes. The entity id stays the same across stop and start, a name resolves through the entity, and `delete` removes the entity with its running lease, snapshots and name. If the entity store fails, a lease request that needs it is denied with `entity_unavailable`, which is transient. Retry with the same idempotency key.

A caller sees only its own tenant's entities; an id owned by another tenant answers `not_found`.

Grant only the verbs the client uses. A read-only dashboard client needs the list, usage, `released` and `evidence` rows, not the command or file rows.

## Operation ids

Create and restore requests and `pause`, `resume`, `suspend`, `wake` and `delete` accept an optional `operation_id` (letters, digits, `-` and `_`, up to 128 characters). The broker records the request before acting and stores the terminal outcome, so a client that timed out can:

- repeat the same call with the same id and get the stored outcome back without the action running again;
- read the record with `sandbox.operation.get`, which answers `NotFound` for an id that belongs to another tenant.

Reusing an id for a different request is refused with `operation_conflict`. For operation-backed lifecycle calls, the broker generates an id when omitted and returns it in the response.

`sandbox.operation.capabilities` takes a tenant-scoped request and reports `supports_create_cancellation`. The gateway derives the tenant from its session and accepts an empty payload for this check. Signal-bound SDK creation is refused before dispatch when support is absent.

`sandbox.operation.cancel` takes `tenant`, `operation_id` and optional `compensate_created` (default false). Cancellation intent can arrive before the create request; the broker retains it so a reordered request cannot allocate unnoticed. A `canceled` receipt confirms settled owned cleanup, while `pending` requires retaining the same identity and inspecting or retrying the operation. Other lifecycle cancellation kinds remain unsupported.

Plain cancellation preserves an already-completed operation. With `compensate_created: true`, the broker may retire only its verified newly-owned allocation; a reused lease is preserved. The original successful outcome is immutable. `sandbox.operation.get` can project `compensation: "pending"` or `"cleaned"` beside `state: "succeeded"`, so read both before interpreting the lease's state.

The broker authorization table includes tenant-scoped `OperationGet`, `OperationCancel` and `OperationCapabilities` entries. Restricted NATS credentials still need the exact subject permissions listed above. Provider cleanup subjects such as `sandbox.provider.e2b.cancel_owned.<lease_id>` belong to broker and worker credentials, not clients.

## Replies and `_INBOX.>`

`async-nats` replies to a request on a per-connection inbox under the `_INBOX.` prefix. A request client therefore needs subscribe permission on `_INBOX.>`. To avoid sharing the inbox space between credentials, the application can set a custom inbox prefix on the NATS connection options (`custom_inbox_prefix` in `async-nats`) and then allow only `_INBOX.<tenant>.>`. `SandboxClient::from_client` accepts an already connected client, so this is the application's setting; no sandbox crate sets it.

## Terminal output is per terminal id

`sandbox.terminal.out.<terminal_id>` carries raw PTY output, including echoed keystrokes and typed passwords. It is core NATS, not JetStream, so nothing is persisted. The subject is keyed by terminal id alone, with no tenant or lease token, so a permission of `sandbox.terminal.out.>` lets the holder read every terminal in the cluster. The terminal id is a bearer capability: anyone who may subscribe to the subject and knows or can wildcard the id can read the terminal.

Two consequences:

- Grant subscribe on `sandbox.terminal.out.>` only to credentials that are allowed to open terminals and that are trusted not to listen to other tenants' terminals. NATS cannot narrow it to one tenant because the subject has no tenant token.
- For untrusted or per-tenant clients, do not hand out NATS access to this subject at all. Put them behind `trogon-sandbox-ws-gateway`, which picks the terminal id itself and subscribes to that id's output before it sends the open, so a client cannot name another terminal. The gateway's own credential carries the wide subscribe permission.

See [Open an interactive terminal](../../how-to/sandbox/open-an-interactive-terminal.md).

## Subjects a client must not get

| Subject | Used by | Why not |
| --- | --- | --- |
| `sandbox.provider.>` (dispatch, cancel, run_command and the other worker verbs) | broker and workers | Talking to a worker directly skips the broker's admission, tenant lookup and quota. Publishing here can act on any lease. |
| `sandbox.worker.heartbeat.>` and `sandbox.worker.<id>.snapshot.export` | workers, broker, gateway | Heartbeats feed the worker registry; forged ones can steer placement. The gateway's own credential reads them for the dashboard. The export subject returns a disk capture of a lease, so publishing on it belongs to the snapshot builder and broker credentials alone; a worker credential subscribes only to its own `<id>`. |
| `sandbox.egress.decide.*`, `sandbox.egress.request.*`, `sandbox.egress.violation.*`, `sandbox.fs.violation.*` | egress proxy, workers, decider and audit services | Policy decisions and violation reports for every lease. |
| `sandbox.snapshot.published`, `sandbox.snapshot.deleted` | snapshot publishers, broker, gateway | Events for all tenants. A client could forge or read them. |
| `sandbox.lease.failed.*` | broker | Broker output. |
| `sandbox.credentials.>` and `sandbox.audit.>` | credential broker, auditors | Credential issue, rotate and revoke, and audit streams. |
| `$JS.API.>`, `$KV.>` and the `sandbox-*` KV buckets (`sandbox-broker-kv-snapshot`, `sandbox-lease-name-index`, `sandbox-drive`, `sandbox-lease-usage` and the rest listed in `constants.rs`) | broker | Direct KV access bypasses the broker's rules. |

## Sample permissions block

Placeholder names only. One user per tenant client, with a custom inbox prefix of `_INBOX.tenant_a`.

```text
authorization {
  users = [
    {
      nkey: "<TENANT_A_PUBLIC_NKEY>"
      permissions: {
        publish: {
          allow: [
            "sandbox.lease.request",
            "sandbox.lease.fork",
            "sandbox.lease.get_by_name",
            "sandbox.lease.list",
            "sandbox.lease.reap",
            "sandbox.lease.cancel.*",
            "sandbox.lease.delete.*",
            "sandbox.lease.stop.*",
            "sandbox.lease.pause.*",
            "sandbox.lease.resume.*",
            "sandbox.lease.suspend.*",
            "sandbox.lease.wake.*",
            "sandbox.lease.snapshot.*",
            "sandbox.lease.extend_timeout.*",
            "sandbox.lease.tags.update.*",
            "sandbox.lease.network_policy_update.*",
            "sandbox.lease.usage.*",
            "sandbox.lease.run_command.*",
            "sandbox.lease.poll_command.*",
            "sandbox.lease.kill_command.*",
            "sandbox.lease.stop_command.*",
            "sandbox.lease.restart_command.*",
            "sandbox.lease.read_logs.*",
            "sandbox.lease.write_stdin.*",
            "sandbox.lease.list_processes.*",
            "sandbox.lease.wait_ready.*",
            "sandbox.lease.wait_sandbox_ready.*",
            "sandbox.lease.write_files.*",
            "sandbox.lease.read_file.*",
            "sandbox.lease.file_op.*",
            "sandbox.lease.get_domain.*",
            "sandbox.tenant.usage",
            "sandbox.tenant.limits",
            "sandbox.drive.create",
            "sandbox.drive.list",
            "sandbox.drive.delete",
            "sandbox.snapshot.get",
            "sandbox.snapshot.delete",
            "sandbox.native_snapshot.delete",
            "sandbox.snapshot.tree",
            "sandbox.snapshot.tag.set",
            "sandbox.snapshot.tag.delete",
            "sandbox.snapshot.tag.history",
            "sandbox.named.delete",
            "sandbox.capability.list",
            "sandbox.operation.capabilities",
            "sandbox.operation.cancel",
            "sandbox.operation.get",
            "sandbox.entity.get",
            "sandbox.entity.list",
            "sandbox.entity.delete"
          ]
        }
        subscribe: {
          allow: [
            "_INBOX.tenant_a.>",
            "sandbox.evidence.*.>",
            "sandbox.lease.released.*"
          ]
        }
      }
    }
  ]
}
```

Notes on the block:

- The subscribe rules `sandbox.evidence.*.>` and `sandbox.lease.released.*` are not tenant-scoped, because lease ids carry no tenant. A tenant that must not see another tenant's evidence cannot be isolated by these subjects alone; use the gateway, or one NATS account per tenant with the broker subjects exported and imported.
- Add `sandbox.lease.terminal.*` to publish and `sandbox.terminal.out.>` to subscribe only for clients that may open terminals, per the section above.
- Add `sandbox.lease.create_user.*`, `create_group.*`, `modify_group_membership.*`, `native_handle.*`, `ssh_access.*`, `cancel_command_launch.*` and `rollback.*` only when the client uses them.
