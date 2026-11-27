# The Agent Substrate provider

`trogon-sandbox-substrate-worker` translates sandbox leases into Agent Substrate actors. The adapter targets the
pinned `substrate.ateapi.v1` contract in `rsworkspace/crates/sandbox/trogon-sandbox-substrate-worker/proto/ateapi.proto`.
The broker remains above the substrate: it chooses a provider and owns tenant admission, quotas and lease deadlines.
Other providers can serve other leases in the same application.

```text
Application / agent
        |
        | sandbox SDK / gateway API
        v
Sandbox broker -------- other provider workers
  admission, TTL,
  quota, dispatch
        |
        | NATS lease / command contracts
        v
Substrate worker -------- NATS KV
  translation,             templates, ownership, recovery
  native cleanup
        |                         |
        | gRPC TLS + bearer       | mTLS CONNECT
        v                         v
Substrate Control API       Substrate router
        |                         |
        v                         v
ActorTemplate / Actor ------ trogon-sandbox-actor-agent
        |                    exec, files, Linux users
        v
WorkerPool: gVisor / microVM
```

For operation, see [the runbook](../../how-to/sandbox/run-a-substrate-sandbox.md). For version, wire behavior and
verification boundaries, see [the adapter contract](../../reference/sandbox/substrate-v1-contract.md).

## The worker and broker have different authority

| Decision | Owner |
|---|---|
| Whether a tenant may get a sandbox, quota, lease TTL | Broker |
| Provider selection and declared requirements | Broker |
| Tenant-to-atespace mapping and actor ownership receipt | Worker |
| Whether a service identity may operate on an atespace | Substrate authorization |
| Actor placement and runtime isolation | Substrate |
| Concrete template resources and compatible pool labels | Worker configuration and cluster operator |
| Exec, file operations and Linux user provisioning | Agent inside the actor |
| Lease-end release, native cleanup and reconciliation | Worker, following the broker lifecycle |

The worker's atespace mapping does not replace upstream authorization. The operator must grant the worker the API and
router permissions it needs and scope them appropriately.

## Exec travels through an authenticated tunnel

The Control API has no equivalent to `docker exec`. The worker opens an mTLS connection to the shared router, sends
CONNECT for the actor and agent port, and speaks the agent's HTTP protocol inside that tunnel. API bearer-token
authentication and router client-certificate authentication are separate.

The agent performs command execution, polling, signals, file operations and user provisioning. Workspace cloning
therefore runs inside the actor, whose image must contain `git`. Privileged user provisioning requires the agent to run
as root and the image to contain the relevant Linux tools. Requests for `run_as_user` and `as_user` drop privileges
inside the actor.

The adapter does not expose general SSH attachment. Its single exposed-port limit is an adapter constraint, not a claim
that the substrate has only one hostname or port.

## Templates capture runtime inputs

An actor refers to an `ActorTemplate`, whose golden snapshot can avoid repeating image initialization. The template
name is a hash of the digest-pinned workload and agent images, the sandbox class, the selected size label, the concrete
CPU and memory, the agent port, the volume shape and the snapshot storage location. A resource or runtime change
therefore builds a separate template.

Per-lease commands and environment are applied by the agent after resume. They are excluded from the template so tenant
secrets and commands are not frozen into a shared golden snapshot. Template build coordination uses a NATS KV create so
that exactly one worker drives each build while the others wait.

## Sizing combines limits with pool selection

Templates carry explicit CPU millicores and memory bytes. The adapter keeps the configured size ladder and rounds a
lease's memory request up to the smallest fitting class. It emits that class's concrete memory limit, plus the
configured actor CPU capacity. CPU capacity is distinct from the lease's `cpu_seconds` execution budget.

Scheduling selects pools by `ate.dev/size` and `sandbox.trogon.live/substrate-class`. Operators must apply matching
labels to compatible pools. A request above the largest class is refused. The template limits must fit the selected
pool; a matching label alone does not prove that capacity is available.

## Retention needs evidence of durable memory

Named persistent leases retain their actor on release. The worker requests synchronous suspension and checks the
resulting actor-owned snapshot for `MEMORY` fidelity before reporting retained release success. Missing, downgraded or
pending evidence is not proof of preserved runtime state. Cleanup responsibility stays with the worker when it cannot
settle release safely.

This check matters because `preferred_fidelity` set to `MEMORY` is a ceiling. The server can capture lower fidelity, and
a later resume may cold-boot. A successful retained release is evidence about the observed snapshot, not a promise about
every later restoration. A named actor with a changed template is refused rather than silently replaced.

Anonymous or non-persistent actors are deleted. A server conflict requires reconciliation, not an unguarded retry.

## Recovery preserves custody across failures

The actor index in NATS KV links an actor to the lease that claimed it. Startup recovery checks indexed actors against
the control plane. A missing actor can prune an entry; an unreachable control plane does not prove an actor is gone.

A failed startup recovery is logged and the worker continues to serve. This is an availability choice, not a fail-closed
startup guarantee. A periodic reaper reconciles unclaimed fleet actors.

## Capabilities stay conservative

Native substrate features do not automatically implement the sandbox API:

- Native snapshots remain distinct from portable signed OCI snapshot artifacts. The adapter does not advertise portable
  snapshot, export or restore.
- Explicit sandbox pause and resume and dormancy remain unsupported even though retained lease release uses native
  suspend and resume internally.
- Upstream has egress policy RPCs, but the adapter does not drive them and advertises no egress enforcement.
- Persistent drives, host mounts and remote mounts are not implemented by this adapter. Its actor-local durable
  directory has the actor's lifetime.

## Guest protocol versions are independent

The worker and `trogon-sandbox-actor-agent` exchange `x-sandbox-protocol-version`, `x-sandbox-agent-protocol-version`
and `x-sandbox-agent-min-protocol-version`. These are versions of the guest HTTP protocol, independent of the substrate
contract version.

A missing header is version 0. An incompatible peer fails explicitly. During a deliberate mixed-fleet rollout, the
agent's `SANDBOX_MIN_WORKER_PROTOCOL_VERSION` or the worker's `SANDBOX_MIN_AGENT_PROTOCOL_VERSION` can temporarily admit
version 0. Remove the override after rebuilding images and upgrading workers.
