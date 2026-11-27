# Agent Substrate v1 adapter contract

## Version boundary

| Item | Pinned contract |
| --- | --- |
| Upstream release | `v1.0.1` |
| Source file | `api/ateapi/v1/ateapi.proto`, fetched by tag in `.config/buf/substrate-worker.buf.gen.yaml` |
| Control service | `substrate.ateapi.v1.Control` |
| Guest HTTP protocol | `trogon-sandbox-actor-agent`, versioned independently |

The adapter does not negotiate the beta service or translate beta enum values. A server upgrade requires reviewing this contract and rerunning its regressions.

## API and transport mapping

| Sandbox operation | Adapter and native operation | Constraint |
| --- | --- | --- |
| Cold allocation | Ensure atespace and template, CreateActor, ResumeActor | Template ready and actor running before guest readiness |
| Warm named allocation | Locate held actor, ResumeActor | Template must match; `resumed=false` can be idempotent success |
| Commands, files, users | Guest HTTP through router CONNECT | Verified router mTLS, agent protocol compatibility |
| Retained release | SuspendActor with synchronous mode, inspect snapshots | Latest actor-owned generation must show durable MEMORY completion |
| Purge | Settle native state, then DeleteActor | Held UID and observed version preconditions |
| Template replacement | Guarded DeleteActorTemplate | Native identity and version preconditions |
| Portable snapshots | Unsupported | Native snapshots are not signed portable OCI artifacts |
| Network policy update | Unsupported | Available native egress RPCs are not integrated |
| Explicit pause and resume, dormancy | Unsupported | Internal retained suspension does not advertise these capabilities |

The API connection uses TLS with a configured CA and bearer token. The worker rereads the token on every call. The router connection uses its own client certificate, key, trust bundle and explicit server name. Router files are reread on each connection, including Kubernetes' combined credential bundle. Missing or invalid TLS material fails the request without plaintext fallback. The production worker requires router TLS; a separate explicit flag permits local plaintext fixtures. The router selects an actor by the `ate-target-actor` header on a CONNECT request.

## Actor lifecycle

```text
lease request
     |
     v
ensure tenant template --> create / locate held actor
                                   |
                                   v
                               ResumeActor
                                   |
                                   v
                         RUNNING + guest ready
                          /                \
             retained Release              Purge
                     |                       |
          synchronous SuspendActor       settle actor
                     |                       |
          verify newest actor-owned      guarded delete
          MEMORY snapshot durable        held UID + version
                     |                       |
          retained release success       actor absent
```

A v1 suspend request defaults to asynchronous mode. An already-suspended actor can be returned before durable storage completes even with synchronous mode, so the adapter checks snapshot evidence and polls pending durability within its deadline. Unknown or lower fidelity cannot satisfy retained release. A crashed named persistent actor remains under cleanup custody instead of being automatically destroyed; upstream may still have recoverable snapshot history.

`preferred_fidelity=MEMORY` requests the highest allowed fidelity. It does not force the server to capture memory or guarantee that a later resume restores process state. Successful release proves only the observed durable snapshot condition. Server deletion conflicts never authorize deleting a replacement under the same name.

This completion condition belongs to the provider's cancel operation. The broker and SDK stop acknowledgement can precede worker completion and is not a durable-snapshot receipt. Deployment acceptance must check the native actor and snapshot state rather than rely on that acknowledgement or a fixed sleep.

Only native deletion in this mapping carries atomic UID and version preconditions. V1 ResumeActor and SuspendActor accept a name reference without those guards; the router's CONNECT target is also addressed by atespace and actor name. Ownership checks around these calls can detect replacement but cannot make a read and a native lifecycle mutation atomic. Deployments must prevent competing controllers from replacing actors behind this worker; stronger upstream lifecycle fencing remains a separate contract requirement.

## Template and scheduling contract

Workload and agent images must be digest-pinned. Templates specify positive CPU millicores below 1,000,000 and nonzero memory bytes. The adapter uses the selected size class's MiB capacity and configured actor CPU millicores, with at least 128 MiB for microVMs. CPU capacity is not the lease execution budget. Explicit per-lease `vcpu_millis` requests are refused because that sizing capability is not advertised by this adapter.

The selector combines `ate.dev/size` and `sandbox.trogon.live/substrate-class`. Pool labels and capacity are operator inputs; the adapter cannot prove that the cluster honors a label.

The content key includes the API discriminator, concrete resource limits, images, class, size label, agent port, snapshot location and volume shape. Template cache coordination is scoped to the atespace, and cached readiness must be checked against that atespace's native resource before use.

Omitting a per-template snapshot location uses the server's storage default. Supplying one requires the server's `PerTemplateSnapshotLocation` preview gate. The adapter does not silently enable upstream preview features.

## Executable traceability

Run from `rsworkspace`:

```sh
cargo test -p trogon-sandbox-substrate-worker --test substrate_v1
cargo test -p trogon-sandbox-substrate-worker --test router_tls
cargo test -p trogon-sandbox-substrate-worker --test template_tenant_scope
cargo test -p trogon-sandbox-substrate-worker -p trogon-sandbox-admin -p trogon-sandbox-client
```

| Boundary | Evidence |
| --- | --- |
| Official service route, actor numeric states, idempotent resume, explicit synchronous suspend | `tests/substrate_v1.rs`, independent wire messages |
| Router CA, name and client identity, CONNECT routing, rotation and no plaintext fallback | `tests/router_tls.rs`, generated PKI and a local guest HTTP service |
| Tenant-scoped template coordination and native readiness readback | `tests/template_tenant_scope.rs` |
| Resource validation, template shape and key, release custody, recovery and cleanup | Worker crate suite |

These tests establish local adapter behavior against controlled peers. They are not upstream certification or live cluster acceptance.

## Remaining deployment evidence

A deployment receipt must bind the server release, pool configuration, worker and agent image digests, authorization policy and test results. Exercise create, exec, files and users, retained release and resume with observed snapshot fidelity, purge, replacement conflicts, certificate rotation, tenant denial, worker restart and orphan cleanup.

Startup recovery warns and continues on recovery error. Latency profile values are estimates.

Failed retention preserves the current worker's active custody and original index entry. It does not persist a separate failed-retention intent. After a restart, suspended or crashed entries that were never marked retained are not adopted as active leases and can become eligible for cleanup. Rely on retained state only after release succeeds.
