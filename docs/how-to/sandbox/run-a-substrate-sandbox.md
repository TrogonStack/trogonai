# Run a sandbox on Agent Substrate v1

This guide targets Agent Substrate `v1.0.1`, the `substrate.ateapi.v1` contract pinned in `.config/buf/substrate-worker.buf.gen.yaml`. It describes the adapter configuration. Validate it against your intended cluster before rollout. The contract the adapter relies on is in [Agent Substrate v1 adapter contract](../../reference/sandbox/substrate-v1-contract.md).

## Prepare the cluster and images

The worker needs NATS with JetStream, access to the Substrate API and router, authorized identities, compatible worker pools and server-side snapshot storage.

- API endpoint: normally `https://api.ate-system.svc:443`, with its CA trusted by the worker. Use a projected ServiceAccount token whose audience is the API service.
- Router: the default is `atenet-router.ate-system.svc:8444`, with the matching TLS server name, trust bundle and a client certificate issued to the worker pod.
- Authorization: grant both the API token principal and the router certificate principal the access they need. A token principal and a certificate principal are distinct even when they represent the same ServiceAccount.
- Worker pools: label each compatible pool with `ate.dev/size=<class>` and `sandbox.trogon.live/substrate-class=gvisor` or `microvm`. Check capacity against the actor limits. Use the pinned upstream release's pool schema.
- Images: provide digest-pinned workload and agent references. The workload image needs `git` and must run the configured agent binary (`trogon-sandbox-actor-agent`). The agent binds `8110` by default. Linux user provisioning additionally needs root, bash and the user and group management tools.

Mount the credential files read-only. The pod certificate bundle contains both private key and certificate chain; pass that same file as the router client certificate and key. Router credential and trust files are reloaded for each connection, and API bearer tokens for each call. The API TLS CA is loaded at connection setup, so reconnect or restart the worker when rotating that CA. Do not print credential contents when checking configuration.

## Start the worker

Set the variables to your deployment's endpoints, file paths and image references. The router server name should match its serving certificate, normally `atenet-router.ate-system.svc`.

```sh
cargo run -p trogon-sandbox-substrate-worker -- \
  --nats-url "$SANDBOX_NATS_URL" \
  --ate-api-endpoint "$SANDBOX_SUBSTRATE_API_ENDPOINT" \
  --tls-ca-file "$SANDBOX_SUBSTRATE_TLS_CA_FILE" \
  --bearer-token-file "$SANDBOX_SUBSTRATE_BEARER_TOKEN_FILE" \
  --router-endpoint "$SANDBOX_SUBSTRATE_ROUTER_ENDPOINT" \
  --router-tls-ca-file "$SANDBOX_SUBSTRATE_ROUTER_TLS_CA_FILE" \
  --router-tls-client-cert-file "$SANDBOX_SUBSTRATE_ROUTER_TLS_CLIENT_CERT_FILE" \
  --router-tls-client-key-file "$SANDBOX_SUBSTRATE_ROUTER_TLS_CLIENT_KEY_FILE" \
  --router-tls-server-name "$SANDBOX_SUBSTRATE_ROUTER_TLS_SERVER_NAME" \
  --agent-image "$SANDBOX_SUBSTRATE_AGENT_IMAGE" \
  --size-class small=2048,medium=8192,large=32768 \
  --actor-cpu-millicores 1000 \
  --max-concurrent-leases 64
```

`--tls-server-name` overrides API certificate name verification when dialing through a different address. The API token file defaults to Kubernetes' projected ServiceAccount token path. Router TLS paths are required unless `--allow-insecure-router` is explicitly selected for a local test double. An insecure router configuration never represents production v1 acceptance.

| Flag | Environment variable | Default and meaning |
| --- | --- | --- |
| `--sandbox-class` | `SANDBOX_SUBSTRATE_SANDBOX_CLASS` | `gvisor`; fallback when the lease does not require an isolation tier |
| `--size-class` | `SANDBOX_SUBSTRATE_SIZE_CLASS` | `small=2048,medium=8192,large=32768`; label and memory MiB mapping |
| `--actor-cpu-millicores` | `SANDBOX_SUBSTRATE_ACTOR_CPU_MILLICORES` | `1000`; actor CPU capacity, separate from the `cpu_seconds` budget |
| `--agent-port` | `SANDBOX_SUBSTRATE_AGENT_PORT` | `8110`; agent listener inside the actor |
| `--snapshots-location` | `SANDBOX_SUBSTRATE_SNAPSHOTS_LOCATION` | Empty uses the server storage default; a per-template URI requires the upstream `PerTemplateSnapshotLocation` preview gate |
| `--template-build-timeout-seconds` | `SANDBOX_SUBSTRATE_TEMPLATE_BUILD_TIMEOUT` | `600` |
| `--resume-timeout-seconds` | `SANDBOX_SUBSTRATE_RESUME_TIMEOUT` | `120` |
| `--max-concurrent-leases` | `SANDBOX_SUBSTRATE_MAX_CONCURRENT_LEASES` | `64`; advertised admission ceiling, not proof of pool capacity |
| `--allow-insecure-router` | `SANDBOX_SUBSTRATE_ALLOW_INSECURE_ROUTER` | `false`; explicit plaintext local test configuration |

The TLS flags use the environment names shown in the command above. Explicit worker identity uses `SANDBOX_SUBSTRATE_WORKER_ID`; durable dispatch fencing uses `SANDBOX_DISPATCH_FENCE_FILE`. Scheduling relies on matching class and size labels.

## Select resources and prebuild templates

The worker rounds requested memory up to the smallest fitting configured class and uses that class's declared MiB as the template memory limit. A 3 GiB request with the default ladder gets 8 GiB. A request above the ladder is refused. MicroVM classes require at least 128 MiB. CPU capacity must be positive and below 1,000,000 millicores. The worker does not advertise per-lease CPU sizing. A lease that explicitly sets `vcpu_millis` is refused; configure actor capacity with the worker flag.

Prebuild per tenant, image, class and concrete resources. Set `SANDBOX_WORKLOAD_IMAGE` to the digest-pinned workload image reference, for example `<registry>/<repository>@sha256:<digest>`. The admin command reaches NATS through its global `--nats-url` flag (`NATS_URL`).

```sh
cargo run -p trogon-sandbox-admin -- substrate prebuild \
  --tenant acme \
  --image "$SANDBOX_WORKLOAD_IMAGE" \
  --nats-url "$SANDBOX_NATS_URL" \
  --ate-api-endpoint "$SANDBOX_SUBSTRATE_API_ENDPOINT" \
  --tls-ca-file "$SANDBOX_SUBSTRATE_TLS_CA_FILE" \
  --bearer-token-file "$SANDBOX_SUBSTRATE_BEARER_TOKEN_FILE" \
  --agent-image "$SANDBOX_SUBSTRATE_AGENT_IMAGE" \
  --sandbox-class gvisor \
  --size-class small \
  --memory-mib 2048 \
  --actor-cpu-millicores 1000
```

Use the same resource values, agent image and port, and optional snapshot location as the worker. The image must be pinned by a complete sha256 digest. Admin `--size-class` is a single label; `--memory-mib` is that class's capacity, not an arbitrary lease request. Templates are not shared across tenants, so warming an image for one tenant does nothing for another. The command builds the template in the tenant's atespace through the same golden-snapshot build the worker uses.

## Lease and retain actors

Ask the broker for `provider: substrate`. Lease commands and environment are applied by the agent rather than frozen into a template. Lifecycle events show template initialization as `PullingImage`, followed by allocation and runtime readiness.

A named persistent lease requests retained actor state at release. The worker requests synchronous suspension and requires a completed durable MEMORY snapshot before reporting retained release success. Upstream may downgrade snapshot fidelity or cold-boot on resume, so memory preservation is not an unconditional service guarantee. A changed template under an existing named actor is refused. Non-persistent or anonymous actors are purged with native UID and version guards.

## Verify on the intended cluster

A deployment that previously ran a beta Substrate server needs a separate server and workload migration plan. This adapter does not migrate upstream storage, access policies, pools or native snapshots. V1 template keys are distinct, so prebuild them with the new limits and selectors. Existing named actors that refer to old templates are refused; preserve their state through an explicit operator migration rather than deleting them to satisfy the new key. Do not point the adapter at a beta API or relabel old image digests as v1 images.

After local contract tests pass, obtain a separate deployment receipt covering:

- API token authorization and router mTLS authorization, including certificate rotation and denied cross-tenant access.
- Template admission, golden snapshot readiness and class and resource placement using rebuilt digest-pinned images.
- Cold create, command, file and user operations, retained release and resume, observed snapshot fidelity, and non-persistent deletion.
- Conflicts, replacement actor protection, worker restart and recovery, and orphan cleanup.

Running the local tonic and TLS fixtures does not establish these cluster properties. Explicit pause and resume, dormancy, portable OCI snapshots, SSH, egress enforcement and persistent drives remain outside this adapter's advertised capabilities.
