# E2B runtime contract

This reference describes the E2B worker implementation (`rsworkspace/crates/sandbox/trogon-sandbox-e2b-worker`) and its operator boundaries. [Run the E2B worker](../../how-to/sandbox/e2b-worker.md) covers connection and template settings.

## General file operations

The profile declares general file operations, search, archive transfers and directory-transfer checksums. Metadata and listings use native `envd` filesystem RPCs. Derived operations preserve the broker's options through bounded byte streams and guest tools.

| Operation | Contract and limits |
| --- | --- |
| Stat and list | Native metadata, relative sorted listings, bounded recursion and entry counts; nested symlinks are not traversed. |
| Read and write | Binary ranges and chunks up to 16 MiB per operation; offsets, truncation, modes and parent-creation intent are honored. Large files can be transferred in chunks. |
| Edit | Exact occurrence counts and an original-byte precondition before atomic replacement; input and replacement are bounded to 16 MiB. Coordinate independent guest writers when an application needs a larger transaction. |
| Search | Glob or literal and regex content search, result and file-size limits, owned temporary staging, and an aggregate 256 MiB scan ceiling reported through truncation. |
| Mkdir, remove, rename | Preserve parent creation, nonrecursive removal and no-overwrite intent. Destructive root paths and malformed paths are refused. |
| Archive get and put | Plain files and directories, bounded entries and a 64 MiB tar ceiling. Incoming traversal, links and special devices are rejected before guest mutation. Existing destination symlink components are refused. |
| Checksum | SHA-256 over the requested range, streamed without the read-result ceiling or whole-file buffering. |

Temporary staging has explicit cleanup. Operation errors retain their primary failure when cleanup also fails. Missing template tools return a failure instead of silently dropping options. The source modules are under `rsworkspace/crates/sandbox/trogon-sandbox-e2b-worker/src/file_ops`.

## Preview access after worker restart

Native recovery reads the sandbox's effective `network.allowPublicTraffic` setting. Known public previews remain available without a traffic credential. Protected previews and previews with unknown privacy return an explicit denial when the current lease has no usable traffic credential. The exact native GET response does not recover the traffic token returned at creation, and the worker does not persist that token to restore preview access.

Preview responses use the current lease's credentials and native identity. A credential refresh on that lease remains usable; a request admitted for an earlier lease registration cannot expose a replacement sandbox.

## Command and creation cancellation

The E2B profile earns `supports_command_cancellation`, projected to the SDK feature `command_cancellation`. An acknowledged detached execution can be canceled through `Command.cancel`, which sends KILL only to that execution and waits for validated completion evidence. The worker checks the recorded PID and start identity and the native process group before supplying terminal evidence. Signal delivery, a missing execution or a lost reply cannot establish completion.

The current termination proof covers the recorded process group. A descendant that creates a separate session can survive that cancellation. Complete command-family termination must preserve unrelated executions on the same sandbox.

SDK signal-bound commands also require the `command_launch_cancellation` feature so cancellation remains possible when the start reply is lost. The E2B profile keeps `supports_command_launch_cancellation` unsupported, so the SDK refuses those signal requests before launch. E2B does not advertise named processes.

Targeted production workers check effective Require signature and epoch enforcement and a healthy persistent dispatch fence before preparing or releasing fresh controlled work. Missing, replaced, unreadable or truncated journals refuse fresh effects, as do recorded append or sync failures. Restoring the original journal identity can recover a path mismatch; an observed write or sync failure stays latched. Existing running replay and exact cancellation remain available under their retained ownership.

The signed worker RPC `sandbox.provider.e2b.cancel_owned.<lease_id>` takes an exact tenant, lease and native sandbox identity. A current holder fences the lease to cleanup-only access and persists that identity in `sandbox-e2b-lease-cleanup` before any DELETE. The worker accepts disposal only after an exact sandbox GET returns 404 and an `Absent` receipt is durably committed. An acknowledged DELETE with a still-visible sandbox remains pending. A repeated `Absent` receipt returns `Cleaned` without another DELETE.

Cleanup receipts use bounded compare-and-set claims lasting twice the configured HTTP timeout plus five seconds. A reconstructed provider can retry only a matching durable cleanup record; a worker with neither a matching held lease nor a matching receipt stays silent. Storage failure refuses deletion before the side effect. Workers sharing this journal must use the same E2B account and API endpoint. These receipts authorize cleanup only and do not restore general access to a lease.

Cleanup and absence receipts have no automatic age-based expiry. Preserve them while their identities remain admissible; an expired claim only permits another retry under the retained authority. See [Cleanup authority and receipt retention](./cleanup-receipt-retention.md) for storage requirements and namespace retirement.

Create and restore cancellation records broker intent before request reordering can hide an allocation. Signal-bound SDK creation opts into compensation of a verified newly-owned late create; reused leases remain preserved. Compensation retains its own durable cleanup progress without rewriting the original successful operation. Client operation status exposes `compensation: "pending"` or `"cleaned"`; only confirmed provider disposal and broker settlement produce a canceled receipt. See [Operation ids](./client-nats-permissions.md#operation-ids).

Generic lease cancellation on a configured worker uses the same durable cleanup journal: the held lease supplies the exact tenant and sandbox identity, intent is persisted before DELETE, and confirmed absence is persisted before the lease is forgotten. A later owned cleanup request can replay that receipt after worker reconstruction without another DELETE. Storage failure retains cleanup-only custody and refuses deletion. A standalone provider without a configured JetStream context still requires exact GET 404 before forgetting, but does not establish a durable cleanup receipt.

Receipt retention and safe garbage collection remain operator boundaries. Permanently lost command acknowledgements and cancellation of other in-flight lifecycle kinds are outside the contract.

## Worker restart authority

Adopting a retained native sandbox requires signature verification in `require` mode and an intact, file-recovered dispatch fence. Its tenant, worker, lease, attempt and nonzero epoch must match the retained assignment before native GET or active-lease insertion. A current in-memory entry does not establish this independent recovery authority.

Persist the dispatch ledger, its fence journal and the JetStream assignment records together. Missing, malformed, truncated or mismatched recovery evidence refuses readiness for occupied assignments and preserves their custody. Legacy fence records without the worker binding remain readable for request fencing, but cannot authorize native adoption. They require trusted operator reconciliation; assignment fields and request defaults cannot supply the missing authority. Restart recovery does not store command arguments, environment values or provider credentials in the fence.

## Native memory snapshots

Capture calls `POST /sandboxes/{id}/snapshots` with `memory: true` and no provider name, creating a fresh template for each capture. The worker protocol's `native_name` becomes a logical host label, limited to 128 UTF-8 bytes without control characters. It is never sent as a reusable provider alias.

The worker accepts a fresh raw template ID tagged `:default` with an empty `names` list, then resolves `GET /templates/{rawTemplateID}` to the exact template and a unique associated ready build UUID. A namespaced alias, nonempty alias list or malformed create response does not establish exclusive cleanup ownership.

The returned E2B `NativeSnapshotRef` contains a host-generated opaque ID. Its version 3 manifest binds that reference to the canonical raw template ID, immutable build UUID and logical label. Restore creates a sandbox through `POST /v2/sandboxes` with `templateID` set to `{rawTemplateID}:{buildUUID}`. The captured build remains fixed if the provider's default tag changes. OCI export, OCI snapshot and tag restore, and in-place rollback are unsupported.

A capture reconnects its source and refreshes its environment and traffic credentials before reporting success. Source request handles share the environment credential state, including an already parked watch poll. A restored sandbox receives independent credential state.

Execution launches and capture share a gate scoped to the source lease. A capture waits for that lease's current launch registration and keeps later launches behind its inventory boundary. Commands on other leases continue independently, including leases belonging to the same tenant.

The worker records each detached wrapper's PID and Linux start ticks before releasing the command. Snapshot metadata retains those identities, users, execution IDs, output budgets and deadline provenance. Restore checks the persisted owner and reference and the restored process identity before installing execution records. Source and fork registries use `(lease_id, execution_id)` bindings, so releasing one does not remove the other's identity.

Version 3 manifests distinguish an explicit lease lifetime, a finite deadline and a known terminal execution. Finite records retain the remaining budget and original host expiry; restore clamps the saved budget rather than starting a fresh full timeout. Version 1 and version 2 records remain readable, but restore refuses them before creating a sandbox because they lack the immutable provider capture binding. Recapture or an explicitly verified migration is required; guest files cannot supply missing host authority.

Finite detached commands have a guest watchdog and active host supervision. The watchdog bounds runtime from guest arming and checks the original PID, start ticks, process group and session. Host supervision retains the original expiry for reconciliation and restore admission. After whole-worker loss, the guest backup does not promise an exact cutoff on the host wall clock. A successful signal alone is insufficient for a completed timeout result; unconfirmed termination remains an explicit error with cleanup ownership retained.

Process inventory lists host-owned entries. Guest filenames, PID markers and arbitrary process scans cannot establish an execution ID or snapshot ownership. Unknown, expired, malformed or foreign metadata fails closed. A failed adoption attempts deletion of the newly created sandbox. If that deletion fails, the worker retains the exact sandbox solely for cleanup: commands, native handles and capture remain unavailable, while cancel and authority purge can retry disposal.

Capture checks manifest and custody storage before calling the provider. After a fresh raw template identity is acknowledged, the worker records that exact identity before reconnecting the source or resolving the immutable build. A failure enters durable cleanup custody and attempts `DELETE /templates/{rawTemplateID}`. Cleanup is complete only after an exact template GET returns 404. An acknowledged DELETE with a still-visible template remains pending. Failed compensation preserves the primary failure and the owned template identity.

Native retirement uses `sandbox.native_snapshot.delete`, Rust's `SandboxClient::native_snapshot_delete`, or the gateway tool `sandbox_native_snapshot_delete`. The TypeScript client exposes `nativeSnapshotDelete(reference)` and `Sandbox.deleteNativeSnapshot(client, reference)`. This is separate from OCI-digest `snapshot_delete`. The broker authenticates the tenant and persists deletion intent before sending a signed provider request. E2B validates its host-owned reference and canonical binding before acting. Foreign and unknown references return `not_found`; other providers return `unsupported`.

Deletion intent blocks new restore admission, including after a broker or worker restart. A restore already admitted may complete in its independently owned sandbox. Deletion does not stop the source or existing forks. Expired references remain eligible for retirement. Legacy manifests without an immutable binding remain refused rather than treating a public ID as a vendor deletion target.

E2B refuses template deletion while a running restored sandbox uses it. The caller receives a backend refusal and durable cleanup remains pending; the fork continues to run. Release the dependent sandbox when the application is finished with it, then retry retirement or let bounded recovery settle it. A repeat after recovery has confirmed absence returns `not_found`.

Workers share a queue subscription for native deletion. Durable compare-and-set claims prevent overlapping cleanup attempts across workers. Recovery runs at startup and periodically, examines at most 128 custody-stream sequences per pass, and stops automatic vendor attempts after eight persisted attempts, with exponential delays. Exhausted records retain custody for reconciliation; an explicit owned deletion request can make another bounded attempt. A successful retirement leaves a durable deleted record, so late capture completion cannot reactivate the reference.

Capture claims last four configured HTTP timeouts plus 60 seconds; cleanup claims last two timeouts plus five seconds. Recovery waits for the capture claim to expire, then preserves a matching committed version 3 manifest or attempts exact cleanup. A capture intent with no acknowledged canonical raw identity remains quarantined and cannot authorize deletion. Provider creation and host persistence are not atomic: a lost or unparseable create response, or process loss before the raw identity is persisted, still needs external reconciliation. An alias or inventory difference never grants cleanup authority.

## Host manifest storage

Native operations require JetStream. The normal worker entry point supplies its NATS context and creates or opens the manifest KV bucket `sandbox-e2b-snapshot-executions` and custody bucket `sandbox-e2b-snapshot-custody`, each backed by its `KV_` stream. Acknowledged owned lease cleanup uses the separate bucket `sandbox-e2b-lease-cleanup`. There is no in-memory fallback for native ownership or cleanup receipts.

Workers sharing these buckets and the E2B provider queue must use the same E2B account and API endpoint. Isolate separate provider accounts in separate NATS accounts or stacks.

Enable native retirement after all brokers and E2B workers in that account are updated. Earlier binaries do not honor the deletion-intent records.

The worker's NATS account needs permission to create, update and read both snapshot KV streams, publish writes under `$KV.sandbox-e2b-snapshot-executions.>` and `$KV.sandbox-e2b-snapshot-custody.>`, and receive JetStream replies. Recovery reads bounded sequences from the custody stream. Use the JetStream API and reply permissions required by your NATS account policy; bucket pre-provisioning alone does not remove create and update calls. These credentials belong to the host worker and must not be injected into guest environments.

The lease-cleanup bucket needs KV lookup, read and write permissions under `$KV.sandbox-e2b-lease-cleanup.>` and first-use bucket creation permission when it is absent. This store opens an existing bucket before attempting creation. Each tenant-scoped receipt binds its exact lease and canonical sandbox id, cleanup or absence phase and claim expiry, within a 4096-byte limit.

Persist and back up the manifest, custody and lease-cleanup buckets. Metadata keys are tenant scoped. Version 3 manifests bind the tenant, opaque public reference, canonical template ID and immutable build UUID, logical label and execution inventory. A manifest is limited to 256 detached executions and 60 KiB; a custody record is limited to 4096 bytes. Custody retains attempt counts, retry times and capture, cleanup and deleted state. Command arguments, environment values and provider credentials are not stored in these ownership records.

Losing the manifest causes restore refusal even if E2B still retains the snapshot. Recovery requires trusted host metadata; guest execution directories do not supply replacement authority. The optional dispatch-fence file is separate from this bucket.
