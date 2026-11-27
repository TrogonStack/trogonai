# trogon-sandbox-substrate-worker

Serves sandbox leases as Agent Substrate actors.

The worker queue-subscribes to substrate lease dispatches and fulfills each lease as an actor created through the
`ateapi.Control` gRPC service of `ate-api-server`. That service has no exec, attach or port-forward call, so every
command, file transfer and terminal byte travels over HTTP to the in-actor agent (`trogon-sandbox-actor-agent`),
reached through the substrate router's CONNECT-tunnel ingress. The router selects an actor by the `ate-target-actor`
header and resumes it on connect, so exec works against a suspended actor.

## How a lease runs

1. The tenant maps to an atespace and the lease (or its name) to an actor name.
2. An actor template is built for the lease's digest-pinned image, size class and isolation class, once per version,
   through a golden-snapshot build. Templates are indexed in JetStream so workers share them.
3. The actor is created or, for a retained named lease, reused after its template and ownership are verified. The
   actor index in JetStream records which worker and claim own each actor.
4. The actor resumes, the agent reports ready, the lease is bound to it, and the workspace is materialized.
5. Cancel suspends the actor, or deletes it when the lease was not named or persistent. Purge deletes it.

At startup the worker adopts the actors its fleet already owns and prunes the ones it no longer should, before it
serves any request.

## Configuration

| Flag | Environment | Default |
| --- | --- | --- |
| `--nats-url` | `SANDBOX_NATS_URL` | `nats://127.0.0.1:4222` |
| `--ate-api-endpoint` | `SANDBOX_SUBSTRATE_API_ENDPOINT` | required |
| `--tls-ca-file` | `SANDBOX_SUBSTRATE_TLS_CA_FILE` | plaintext |
| `--tls-server-name` | `SANDBOX_SUBSTRATE_TLS_SERVER_NAME` | the endpoint host |
| `--bearer-token-file` | `SANDBOX_SUBSTRATE_BEARER_TOKEN_FILE` | the pod's projected service-account token |
| `--router-endpoint` | `SANDBOX_SUBSTRATE_ROUTER_ENDPOINT` | the in-cluster router address |
| `--router-tls-ca-file`, `--router-tls-client-cert-file`, `--router-tls-client-key-file`, `--router-tls-server-name` | `SANDBOX_SUBSTRATE_ROUTER_TLS_CA_FILE`, `SANDBOX_SUBSTRATE_ROUTER_TLS_CLIENT_CERT_FILE`, `SANDBOX_SUBSTRATE_ROUTER_TLS_CLIENT_KEY_FILE`, `SANDBOX_SUBSTRATE_ROUTER_TLS_SERVER_NAME` | mutual TLS is required |
| `--allow-insecure-router` | `SANDBOX_SUBSTRATE_ALLOW_INSECURE_ROUTER` | off; for local test doubles only |
| `--agent-image` | `SANDBOX_SUBSTRATE_AGENT_IMAGE` | required, digest-pinned |
| `--agent-port` | `SANDBOX_SUBSTRATE_AGENT_PORT` | `8110` |
| `--sandbox-class` | `SANDBOX_SUBSTRATE_SANDBOX_CLASS` | `gvisor` |
| `--size-class` | `SANDBOX_SUBSTRATE_SIZE_CLASS` | `small=2048,medium=8192,large=32768` |
| `--actor-cpu-millicores` | `SANDBOX_SUBSTRATE_ACTOR_CPU_MILLICORES` | `1000` |
| `--snapshots-location` | `SANDBOX_SUBSTRATE_SNAPSHOTS_LOCATION` | empty |
| `--template-build-timeout-seconds` | `SANDBOX_SUBSTRATE_TEMPLATE_BUILD_TIMEOUT` | `600` |
| `--resume-timeout-seconds` | `SANDBOX_SUBSTRATE_RESUME_TIMEOUT` | `120` |
| `--worker-id` | `SANDBOX_SUBSTRATE_WORKER_ID` | generated per process |
| `--max-concurrent-leases` | `SANDBOX_SUBSTRATE_MAX_CONCURRENT_LEASES` | `64` |
| `--dispatch-fence-file` | `SANDBOX_DISPATCH_FENCE_FILE` | in memory |

The bearer token file is read on every call, so a rotated projected token is picked up without a restart.

## The wire contract

`proto/ateapi.proto` is the pinned `substrate.ateapi.v1` contract. Its client-only bindings are checked in at
`src/proto/substrate.ateapi.v1.rs`, and `ate_client` is the single place where `tonic::Status` is classified. To move
to a newer contract, follow `proto/README.md`: replace the file, regenerate the bindings, rerun the tests, and re-read
`ate_client` if a `Control` call changed shape.

## What a lease may not ask for

Remote mounts, persistent drives, privileged access, a user override, more than one exposed port, host-mount
workspaces and `vcpu_millis` are refused: the actor has no FUSE, its volumes do not outlive it, and CPU capacity is
set by the operator. Memory is a property of the worker pool, so `memory_mib` selects a configured size class.

## Related crates

- `trogon-sandbox-contract`: the lease, spec and command types on the wire.
- `trogon-sandbox-protocol`: subjects, dispatch authentication, the authority guard and the provider trait.
- `trogon-sandbox-actor-agent`: the guest protocol this worker speaks, linked without its HTTP server.
- `trogon-sandbox-observability`: tracing and metrics bootstrap.
