# How to deploy trogon-mcp-gateway as a queue group on Kubernetes

**Diátaxis:** how-to (goal-oriented, imperative steps).

**Audience:** platform SREs deploying `trogon-mcp-gateway` into an existing TrogonStack mesh (NATS, JetStream, STS, SpiceDB, agent registry).

**Related:** [MCP gateway operator overview](mcp-gateway-operator-overview.md) · [Reference queue groups](reference-queue-groups.md) · [Reference NATS headers](reference-nats-headers.md) · [Bootstrap / day-zero](bootstrap-day-zero.md) · [Failure mode matrix](failure-mode-matrix.md) · [Kubernetes controller](k8s-controller.md) (v2 — not required here)

| Placeholder | Example value |
|---|---|
| Kubernetes namespace | `trogon-mcp` |
| NATS subject prefix | `mcp` (`MCP_PREFIX`) |
| Queue group | `mcp-gateway` (`MCP_GATEWAY_QUEUE_GROUP`) |
| JetStream audit stream | `MCP_AUDIT` |
| Gateway replicas | `3` |
| Trust domain (SVID) | `acme.local` |

Replace placeholders before running commands.

---

## Goal

You will run N replicas of `trogon-mcp-gateway` on Kubernetes as competing consumers on `{prefix}.gateway.request.>` under queue group `mcp-gateway`, wired to the Trogon control plane. When you finish, the Phase 1 vertical slice (queue-group ingress, JWT verification, CEL gate, SpiceDB on gated methods, audit to JetStream) is reachable from the cluster and observable in NATS and audit.

---

## When to use this

Use this guide when:

- You are bringing up or scaling the MCP gateway data plane in a cluster that already runs Trogon identity services ([ADR 0017](../adr/0017-product-positioning.md) Option A — co-deployed mesh).
- You need a **static manifest** Deployment (v1 manual deploy) before the K8s controller lands ([k8s-controller.md](k8s-controller.md) is v2 declarative tooling).

Do **not** use this guide when:

- You only need to register a backend MCP server — see [How to integrate a third-party MCP server](howto-integrate-third-party-mcp.md).
- You are authoring policy bundles — see [How to write a CEL-only policy bundle](howto-write-bundle.md).
- You need multi-region failover — see [ADR 0016](../adr/0016-multi-region.md) and [multi-region.md](multi-region.md).
- You want a conceptual tour of subjects and enforcement — see [MCP gateway operator overview](mcp-gateway-operator-overview.md) and [Agent identity overview](overview.md).

---

## Prerequisites

Confirm each item before Step 1. Missing items are called out in [bootstrap-day-zero.md](bootstrap-day-zero.md).

| Prerequisite | Authority | Verify |
|---|---|---|
| NATS account for the gateway with JetStream enabled | [ADR 0007](../adr/0007-on-bus-vs-hybrid.md), [ADR 0011](../adr/0011-nats-auth-callout.md) | Gateway creds connect; publish ACL on `{prefix}.gateway.request.>` only (not `{prefix}.server.>`) |
| JetStream stream `MCP_AUDIT` filtering `{prefix}.audit.>` | [reference-audit-envelope.md](reference-audit-envelope.md), `MCP_GATEWAY_AUDIT_STREAM` | `nats stream info MCP_AUDIT` |
| KV bucket `mcp-gateway-config` | [bootstrap-day-zero.md § Step 0](bootstrap-day-zero.md#step-0--provision-jetstream-assets-once-per-account) | `nats kv info mcp-gateway-config` |
| KV bucket `mcp-trust-bundles` | [integration-touchpoints.md](integration-touchpoints.md) | `nats kv get mcp-trust-bundles acme.local` (PEM) |
| SpiceDB gRPC reachable from the cluster | [trogon-mcp-gateway README](../../rsworkspace/crates/trogon-mcp-gateway/README.md) | `grpcurl` or in-cluster Service on `:50051` |
| STS queue group `trogon-sts` on `mcp.sts.exchange` | [sts-exchange.md](sts-exchange.md) | `nats request mcp.sts.exchange …` from a probe pod |
| Mesh JWKS for ingress JWT verify | [ADR 0006](../adr/0006-mesh-token-signing-keys.md) | HTTPS `MCP_GATEWAY_JWT_JWKS_URI` reachable from pods |
| Container image for `trogon-mcp-gateway` | Block D shipped | Image digest pinned in your registry |
| Day-zero policy pointer (or accept mixed posture) | [ADR 0021](../adr/0021-bootstrap-day-zero.md) | `bundle/active` in `mcp-gateway-config` **(proposed key)** or bootstrap complete |

**Production guardrails:** set `MCP_GATEWAY_SPICEDB_ENDPOINT` (unset ⇒ Phase 1 allow-all on gated methods — dev only). Set `MCP_GATEWAY_JWT_MODE=require` for production. Do not run `MCP_GATEWAY_BOOTSTRAP_POSTURE=permissive` with `MCP_GATEWAY_AGENT_IDENTITY=enforce` **(proposed guard — ADR 0021)**.

---

## Steps

### 1. Provision JetStream and KV (once per NATS account)

If [bootstrap-day-zero.md § Step 0](bootstrap-day-zero.md#step-0--provision-jetstream-assets-once-per-account) is already done, skip to Step 2.

1. Create KV buckets: `mcp-trust-bundles`, `mcp-gateway-config`, `mcp-agent-registry`, `mcp-jwks`, `mcp-sessions` (when session KV is enabled).
2. Create the audit stream (gateway can also call `get_or_create_stream` at boot unless skipped):

```bash
export MCP_PREFIX=mcp
nats stream add MCP_AUDIT \
  --subjects "${MCP_PREFIX}.audit.>" \
  --storage file \
  --retention limits \
  --max-msgs 100000 \
  --defaults
```

3. Publish the SPIFFE trust bundle for your domain:

```bash
export NATS_URL=nats://nats.trogon.svc:4222
export MCP_STS_TRUST_DOMAIN=acme.local
nats kv put mcp-trust-bundles "${MCP_STS_TRUST_DOMAIN}" @/path/to/acme.local.bundle.pem
```

**Expected:** `nats stream info MCP_AUDIT` shows subject filter `mcp.audit.>`; `nats kv get mcp-trust-bundles acme.local` begins with `-----BEGIN CERTIFICATE-----`.

---

### 2. Create namespace, ServiceAccount, and secrets

```bash
kubectl create namespace trogon-mcp
kubectl -n trogon-mcp create serviceaccount trogon-mcp-gateway
kubectl -n trogon-mcp create secret generic trogon-mcp-gateway-nats \
  --from-file=creds=/path/to/gateway-service.creds
kubectl -n trogon-mcp create secret generic trogon-mcp-gateway-spicedb \
  --from-literal=token="${SPICEDB_TOKEN}"
```

Set `MCP_GATEWAY_JWT_JWKS_URI` in the ConfigMap (Step 3) or mount static RSA via `MCP_GATEWAY_JWT_RSA_PUBLIC_KEY_PEM` ([README § Verified JWT](../../rsworkspace/crates/trogon-mcp-gateway/README.md)).

**Expected:** `kubectl -n trogon-mcp get secret` shows NATS and SpiceDB secrets.

---

### 3. Apply ConfigMap and Deployment

All replicas **must** share the same `MCP_GATEWAY_QUEUE_GROUP` ([reference-queue-groups.md §1](reference-queue-groups.md#1-pinned-queue-group-table-wire-format-pin-3)). Default queue group name is `mcp-gateway`; it is **not** prefixed by `MCP_PREFIX`.

**Replica count:** start with **3** replicas for production HA. Queue groups load-balance ingress only — they do not shard reply inboxes ([reference-queue-groups.md §7.1](reference-queue-groups.md#71-what-queue-groups-scale)). Add replicas when gateway CPU or queue lag rises while SpiceDB and backend latency stay flat; adding pods does not fix a saturated backend or SpiceDB ([mcp-gateway-operator-overview.md §7](mcp-gateway-operator-overview.md#7-capacity-model)).

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: trogon-mcp-gateway
  namespace: trogon-mcp
data:
  NATS_URL: "nats://nats.trogon.svc:4222"
  MCP_PREFIX: "mcp"
  MCP_GATEWAY_QUEUE_GROUP: "mcp-gateway"
  MCP_GATEWAY_AUDIT_STREAM: "MCP_AUDIT"
  MCP_OPERATION_TIMEOUT_SECS: "30"
  MCP_GATEWAY_SPICEDB_ENDPOINT: "spicedb.trogon.svc:50051"
  MCP_GATEWAY_JWT_MODE: "require"
  MCP_GATEWAY_JWT_ISSUERS: "https://idp.acme.com"
  MCP_GATEWAY_JWT_JWKS_URI: "https://idp.acme.com/.well-known/jwks.json"
  MCP_GATEWAY_JWT_AUDIENCE: "trogon-mcp-gateway"
  MCP_GATEWAY_AGENT_IDENTITY: "shadow"
  MCP_GATEWAY_CHAIN_RESOLUTION_MODE: "enforce"
  MCP_GATEWAY_REGISTRY_SUBJECT: "mcp.registry.agent.lookup"
  MCP_GATEWAY_IDENTITY_SUB: "trogon-mcp-gateway"
  MCP_GATEWAY_BOOTSTRAP_POSTURE: "mixed"   # (proposed -- ADR 0021)
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: trogon-mcp-gateway
  namespace: trogon-mcp
spec:
  replicas: 3
  selector:
    matchLabels:
      app: trogon-mcp-gateway
  strategy:
    type: RollingUpdate
    rollingUpdate:
      maxUnavailable: 1
      maxSurge: 1
  template:
    metadata:
      labels:
        app: trogon-mcp-gateway
    spec:
      serviceAccountName: trogon-mcp-gateway
      terminationGracePeriodSeconds: 45
      containers:
        - name: gateway
          image: ghcr.io/acme/trogon-mcp-gateway:0.3.0
          args: ["--queue-group", "mcp-gateway"]
          envFrom:
            - configMapRef:
                name: trogon-mcp-gateway
          env:
            - name: NATS_CREDS
              value: /etc/nats/creds
            - name: MCP_GATEWAY_SPICEDB_TOKEN
              valueFrom:
                secretKeyRef:
                  name: trogon-mcp-gateway-spicedb
                  key: token
          volumeMounts:
            - name: nats-creds
              mountPath: /etc/nats
              readOnly: true
          ports:
            - name: probes
              containerPort: 8080
          livenessProbe: { httpGet: { path: /healthz, port: probes }, periodSeconds: 10 }   # proposed
          readinessProbe: { httpGet: { path: /readyz, port: probes }, periodSeconds: 5 }  # proposed
          resources: { requests: { cpu: 500m, memory: 512Mi }, limits: { cpu: "2", memory: 1Gi } }
      volumes:
        - name: nats-creds
          secret:
            secretName: trogon-mcp-gateway-nats
            items:
              - key: creds
                path: creds
```

Apply:

```bash
kubectl apply -f trogon-mcp-gateway.yaml
kubectl -n trogon-mcp rollout status deployment/trogon-mcp-gateway
```

**Expected:** `kubectl -n trogon-mcp get pods -l app=trogon-mcp-gateway` shows `3/3 Running`; logs mention SpiceDB authorization and subscribe on `mcp.gateway.request.>`.

See [integration-touchpoints.md § Environment variables](integration-touchpoints.md#environment-variables-gateway-process) and [trogon-mcp-gateway README](../../rsworkspace/crates/trogon-mcp-gateway/README.md) for the full env matrix. Proposed: `MCP_GATEWAY_BOOTSTRAP_POSTURE` (ADR 0021), probes on `:8080` (`tests/health_probes.rs`). Set `MCP_GATEWAY_ACTOR_TOKEN` when identity is `shadow` or `enforce` ([sts-exchange.md](sts-exchange.md)).

---

### 4. Wire audit and confirm queue-group membership

Audit publishes to `{prefix}.audit.{outcome}.{direction}.{method_root}`; JetStream stream `MCP_AUDIT` retains them ([reference-audit-envelope.md](reference-audit-envelope.md)).

```bash
nats stream info MCP_AUDIT | grep -E 'Subjects|Messages'
nats server report connections --filter="trogon-mcp-gateway"
nats sub "mcp.audit.>" --queue audit-smoke --count 5
```

**Expected:** Stream filter includes `mcp.audit.>`; N gateway connections share `queue=mcp-gateway` on `mcp.gateway.request.>`; smoke traffic yields `mcp.audit.allow.request.tools` or `mcp.audit.deny.request.tools`.

---

### 5. Day-zero posture and shadow mode

Until `tenant_initialised` is true ([ADR 0021](../adr/0021-bootstrap-day-zero.md)), production should **deny** gated `tools/call` / `resources/read` with `-32108` `no_policy` **(proposed)** or mixed bootstrap audit — not allow-all.

- **Default deny:** set SpiceDB + bundle pointer in `mcp-gateway-config` ([ADR 0021](../adr/0021-bootstrap-day-zero.md)); never leave `MCP_GATEWAY_SPICEDB_ENDPOINT` unset in prod.
- **Shadow:** `MCP_GATEWAY_AGENT_IDENTITY=shadow` — validate mesh without hard deny; audit `would_deny` **(proposed)**.
- **Enforce:** `MCP_GATEWAY_AGENT_IDENTITY=enforce` — STS/registry fail CLOSED ([failure-mode-matrix row 13](failure-mode-matrix.md#failure-mode-matrix)).
- **Strict bootstrap:** `MCP_GATEWAY_BOOTSTRAP_POSTURE=strict` **(proposed)** — deny all until initialised.

Complete [bootstrap-day-zero.md §3](bootstrap-day-zero.md#3-bootstrap-sequence) before removing shadow.

**Expected:** Gated smoke without bundle returns deny JSON-RPC, not backend forward; audit shows `deny` or `error`, not silent allow ([failure-mode-matrix.md row 5](failure-mode-matrix.md#failure-mode-matrix)).

---

### 6. Rolling restart safety

Kubernetes rolling updates drain one pod at a time when `maxUnavailable: 1`.

**Phase 1 (today):** The gateway handles backend request/reply on the same NATS `Message.reply` the client attached ([ADR 0009](../adr/0009-reply-correlation.md) — inline path shipped). In-flight `tools/call` on a terminating pod may **timeout** on the client if the pod dies mid-flight ([reply-correlation.md §2.1](reply-correlation.md#21-failure-mode-a--replica-dies-before-reply), [failure-mode-matrix.md row 11](failure-mode-matrix.md#failure-mode-matrix)).

1. `terminationGracePeriodSeconds` ≥ `MCP_OPERATION_TIMEOUT_SECS` + buffer (45s in manifest).
2. Keep `maxUnavailable: 1` and a stable `MCP_GATEWAY_QUEUE_GROUP` ([reference-queue-groups.md §7.3](reference-queue-groups.md#73-nats-message-distribution-policy)).
3. Clients retry after pod loss; mutating calls need app idempotency until dedup ships ([reply-correlation.md §6](reply-correlation.md#6-cross-replica-dedup-contract-proposed)).

**Expected:** `kubectl rollout restart deployment/trogon-mcp-gateway` completes with ready count stable; brief P99 latency bump acceptable; no sustained `-32103` backend_unreachable from zero queue subscribers.

---

## Verify

- [ ] All pods `Running`; `/healthz` and `/readyz` on `:8080` **(proposed — `tests/health_probes.rs`)**
- [ ] N replicas on `mcp.gateway.request.>` with `queue=mcp-gateway` ([reference-queue-groups.md §8.1](reference-queue-groups.md#81-confirm-gateway-replicas-share-the-expected-group))
- [ ] `MCP_AUDIT` message count rises; smoke emits `mcp.audit.allow.request.tools` or `mcp.audit.deny.request.tools`
- [ ] Gated call: `-32100` on SpiceDB deny, `-32107` when PDP down ([failure-mode-matrix.md](failure-mode-matrix.md))
- [ ] In `shadow`, audit shows would-deny before `enforce` ([overview.md](overview.md))

---


## Rollback / undo

```bash
kubectl -n trogon-mcp rollout undo deployment/trogon-mcp-gateway
```

- **Identity:** revert `MCP_GATEWAY_AGENT_IDENTITY` to `shadow`, fix audit `would_deny`, re-enforce after seven clean days ([overview.md](overview.md)).
- **Policy:** repoint `mcp-gateway-config` bundle active pointer ([howto-write-bundle.md § Rollback](howto-write-bundle.md#rollback)) **(proposed hot-swap — ADR 0026)**.
- **Scale to zero:** clients see NATS timeouts ([failure-mode-matrix row 11](failure-mode-matrix.md#failure-mode-matrix)).
- **Stale ZedToken:** rolling restart clears process cache ([failure-mode-matrix row 2](failure-mode-matrix.md#failure-mode-matrix)).

---

## Troubleshooting

| Symptom | Root cause | Fix |
|---|---|---|
| Pods `CrashLoopBackOff` on start | Invalid JWT config when `MCP_GATEWAY_JWT_MODE=require` | Set issuers + JWKS/RSA/HS256 per README; check logs for `MCP_GATEWAY_JWT_*` |
| All gated calls allowed in prod | `MCP_GATEWAY_SPICEDB_ENDPOINT` unset | Set SpiceDB endpoint and token; restart pods |
| `-32107` on gated methods | SpiceDB unreachable from pod network | Fix Service/DNS; check [failure-mode-matrix row 1](failure-mode-matrix.md#failure-mode-matrix) |
| No audit messages in `MCP_AUDIT` | Stream missing or wrong subject filter | Recreate stream with `{prefix}.audit.>`; unset `MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT` in prod |
| Duplicate MCP processing | Replicas using different queue group names | Unify `MCP_GATEWAY_QUEUE_GROUP` on all pods |
| Clients timeout during rollout | In-flight request lost on terminated replica | Increase grace period; retry; see [ADR 0009](../adr/0009-reply-correlation.md) |
| STS `invalid_token` / trust failures | Missing `mcp-trust-bundles/{trust_domain}` | Republish PEM; key must equal trust domain ([failure-mode-matrix row 15](failure-mode-matrix.md#failure-mode-matrix)) |
| Readiness never true **(proposed)** | Bundle pointer empty / NATS down | Complete [bootstrap-day-zero.md](bootstrap-day-zero.md); fix NATS creds Secret |

---

## Related

- [mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md) — topology and capacity
- [reference-queue-groups.md](reference-queue-groups.md), [reference-nats-headers.md](reference-nats-headers.md), [reference-audit-envelope.md](reference-audit-envelope.md) — wire pins
- [failure-mode-matrix.md](failure-mode-matrix.md) (ADR 0024), [bootstrap-day-zero.md](bootstrap-day-zero.md) (ADR 0021), [reply-correlation.md](reply-correlation.md) (ADR 0009)
- [k8s-controller.md](k8s-controller.md) — v2 declarative deploy
- ADRs [0007](../adr/0007-on-bus-vs-hybrid.md), [0011](../adr/0011-nats-auth-callout.md), [0017](../adr/0017-product-positioning.md)
- [howto-integrate-third-party-mcp.md](howto-integrate-third-party-mcp.md), [howto-write-bundle.md](howto-write-bundle.md), [trogon-mcp-gateway README](../../rsworkspace/crates/trogon-mcp-gateway/README.md)

---

*Document type: Diátaxis **how-to**. For CRD-based deploy, wait for [k8s-controller.md](k8s-controller.md). For NATS/MCP identity concepts, start with [overview.md](overview.md).*
