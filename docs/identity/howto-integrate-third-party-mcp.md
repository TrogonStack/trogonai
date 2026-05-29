# How to integrate a third-party MCP server behind the gateway

**Diátaxis:** how-to (goal-oriented, imperative steps).

**Audience:** platform operators integrating a vendor or community MCP server (GitHub, Confluence, custom stdio binary) into an existing Trogon MCP gateway deployment.

**Related:** [MCP gateway operator overview](mcp-gateway-operator-overview.md) · [Registry runbook](registry-operations.md) · [Integration touch-points](integration-touchpoints.md) · [Failure mode matrix](failure-mode-matrix.md) · [A2A SDK contract](sdk.md) · [Bootstrap / day-zero](bootstrap-day-zero.md)

This guide walks through registering a third-party MCP server on the NATS bus, attaching policy, and verifying end-to-end traffic through `{prefix}.gateway.request.{server_id}.*`. The worked example uses:

| Placeholder | Example value |
|---|---|
| Tenant | `acme` |
| MCP `server_id` | `github` |
| Smoke-test agent | `acme/smoke-agent` |
| Allowed tool (smoke) | `search_repositories` |
| Denied tool (negative test) | `delete_repository` |
| NATS subject prefix | `mcp` (`MCP_PREFIX`) |

Replace these with your deployment values before running commands.

---

## Prerequisites

Confirm the following before Step 1. If any item is missing, complete [Bootstrap / day-zero](bootstrap-day-zero.md) or [Registry runbook — Bootstrap](registry-operations.md#bootstrap-fresh-cluster) first.

1. **Gateway running.** `trogon-mcp-gateway` is subscribed to `mcp.gateway.request.>` under queue group `mcp-gateway` (or `MCP_GATEWAY_QUEUE_GROUP` override). See [operator overview § Day-2 checklist](mcp-gateway-operator-overview.md#92-gateway-data-plane).

2. **NATS reachable.** Operators and bridge processes can connect with backend-zone credentials (`NATS_URL`, `NATS_CREDS`, or equivalent). JetStream is enabled for audit and KV.

3. **SpiceDB schema seeded.** MCP object definitions exist (`trogon/mcp_tool`, `trogon/mcp_resource`, `trogon/principal`). Dev seed pattern: `devops/docker/compose/services/spicedb/schema.zed`. You will add tuples for the smoke principal in Step 4.

4. **Third-party MCP artifact.** You have either:
   - a **stdio MCP binary** or container entrypoint (most community servers), or
   - a **native NATS MCP server** built with `mcp-nats`, or
   - a **remote HTTP MCP** endpoint (Streamable HTTP via `mcp-nats-server`).

5. **Bootstrap token for testing.** One issued bootstrap NATS User JWT from the auth callout with publish ACL on `mcp.gateway.request.>` only (not `mcp.server.*`). You will exchange it for a mesh token at Step 6.

6. **Operator tooling:** `nats` CLI, `agctl` (`cargo build -p agctl --release`), and `REGISTRY_VERIFY_KEY` pointing at the registry signer PEM.

7. **Environment defaults:**

   ```bash
   export NATS_URL=nats://127.0.0.1:4222
   export MCP_PREFIX=mcp
   export TENANT=acme
   export SERVER_ID=github
   export AGENT_ID=acme/smoke-agent
   export TRUST_DOMAIN=acme.local
   ```

---

## Step 1 — Inventory the server

Answer these questions **before** touching NATS or the registry. Record answers in your change ticket; they drive ACLs, policy, and trust configuration.

### 1.1 Tool surface

Run the MCP server locally and capture `tools/list` (exact probe varies by vendor). Record tool names, resources/prompts, callback methods (`sampling/createMessage` affects bidirectional policy), and schema stability.

The gateway Phase 1 PDP checks **`tools/call`** and **`resources/read`** only ([operator overview § SpiceDB](mcp-gateway-operator-overview.md#54-spicedb-on-gated-methods)). Inventory the full catalog anyway for Step 4 allow/deny rules.

### 1.2 Authentication the server expects

| Pattern | Bridge implication | Gateway implication |
|---|---|---|
| **None** (local stdio) | Pass env vars to subprocess only | No OAuth profile on backend lane |
| **OAuth / API key** | Sidecar holds secrets; never expose to clients | Optional OAuth at HTTP ingress ([oauth-mcp-integration.md](oauth-mcp-integration.md)); mesh token on NATS path |
| **SPIFFE SVID** | Bridge presents SVID to upstream | Configure trust bundle (Step 5) |

Third-party servers must **not** receive bootstrap NATS JWTs or client OAuth tokens on the backend lane. The gateway mints downstream mesh tokens on egress ([ADR 0003](../adr/0003-bootstrap-vs-mesh-tokens.md)).

### 1.3 Placement topology

Choose one deployment shape:

| Shape | When to use | Trust boundary |
|---|---|---|
| **Sidecar** (recommended for stdio) | Vendor ships stdio-only MCP; one bridge pod per server | Bridge NATS creds limited to `mcp.server.{server_id}.>` subscribe |
| **Central NATS-native** | Server built with `mcp-nats` | Shared backend service account; queue group on server lane |
| **Remote** | SaaS MCP over HTTP | `mcp-nats-server` or **proposed:** `trogon-mcp-bridge` remote adapter |

Record the **trust domain** for any workload that presents an SVID (e.g. `spiffe://acme.local/ns/mcp/sa/github-bridge` → trust domain `acme.local`).

### 1.4 Choose a stable `server_id`

Rules ([mcp-nats subject layout](../../rsworkspace/crates/mcp-nats/README.md#subject-layout)):

- Lowercase slug; no dots in `server_id` (dots belong in method suffixes: `tools.call`).
- Immutable once registered — renaming requires decommission + new id.
- Appears in subjects, SpiceDB resource ids (`github|search_repositories`), and STS audience URIs (`urn:trogon:mcp:backend:acme:github`).

---

## Step 2 — Place the server on the bus

Clients must **never** publish directly to `mcp.server.{server_id}.>`. Only the gateway may forward there ([integration touch-points § NATS transport](integration-touchpoints.md#6-gateway--nats-core-and-jetstream)). Pick **Option A** or **Option B**.

### Option A — Sidecar wrapping (recommended for stdio MCP)

A thin adapter subscribes to `{prefix}.server.{server_id}.>`, translates NATS JSON-RPC to stdio, and returns replies on the request inbox.

**Architecture:**

```text
Client ──► mcp.gateway.request.github.* ──► trogon-mcp-gateway
                                                │
                                                ▼
                                         mcp.server.github.*
                                                │
                     ┌──────────────────────────┘
                     ▼
              [sidecar bridge] ◄──stdio──► [third-party MCP binary]
```

**Run the sidecar** (placeholder command — use `trogon-mcp-bridge` CLI when shipped; until then, compose `mcp-nats` server transport with your subprocess):

```bash
export MCP_PREFIX=mcp
export MCP_SERVER_ID=github
export NATS_URL=nats://127.0.0.1:4222
export NATS_CREDS=/path/to/github-bridge-backend.creds   # subscribe mcp.server.github.> only

# Vendor MCP credentials (never forwarded from clients)
export GITHUB_TOKEN=ghp_xxxxxxxxxxxx

# proposed: trogon-mcp-bridge — stdio sidecar for backend zone
# Replace with the shipped binary when available.
proposed: trogon-mcp-bridge serve-stdio \
  --server-id "${SERVER_ID}" \
  --mcp-prefix "${MCP_PREFIX}" \
  --command "/opt/github-mcp/bin/github-mcp" \
  --env GITHUB_TOKEN

# Interim pattern: run a NATS-native stub that execs stdio (operator-maintained)
# or embed mcp-nats server::connect in an internal wrapper crate.
```

Grant the sidecar **subscribe** on `{prefix}.server.{server_id}.>` only; deny `{prefix}.gateway.request.>` and other servers' backend subjects.

**Announce liveness** (control plane):

```bash
nats pub "mcp.control.discovery.register.${SERVER_ID}" \
  '{"server_id":"'"${SERVER_ID}"'","transport":"stdio-sidecar","ts":"'"$(date -u +%Y-%m-%dT%H:%M:%SZ)"'"}'
```

**Verify the sidecar is on the bus** (direct backend ping — bypasses gateway; ACL test only):

```bash
nats request "mcp.server.${SERVER_ID}.tools.list" \
  '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' --timeout 10s
```

Expect a JSON-RPC `result.tools` array. Fix the sidecar before continuing.

### Option B — Native NATS (server speaks NATS already)

When the MCP implementation uses `mcp-nats` ([crate README](../../rsworkspace/crates/mcp-nats/README.md)), run it as a queue-group consumer on the backend lane.

Subject layout: `mcp.server.{server_id}.{method_dots}` — see [mcp-nats README](../../rsworkspace/crates/mcp-nats/README.md#subject-layout).

**Start the server** (example — adjust for your binary):

```bash
export MCP_PREFIX=mcp
export NATS_URL=nats://127.0.0.1:4222
export NATS_CREDS=/path/to/github-mcp-backend.creds

# If your server embeds mcp-nats directly, use its entrypoint.
# mcp-nats-server bridges HTTP→NATS; native servers use server::connect.
cargo run -p your-github-mcp-nats-server -- \
  --server-id "${SERVER_ID}" \
  --mcp-prefix "${MCP_PREFIX}"
```

**Queue group recommendation:** use one queue group name per logical MCP server so multiple replicas share load without duplicate delivery:

```text
Queue group: mcp-server-{server_id}     # e.g. mcp-server-github
Subscribe:   {prefix}.server.{server_id}.>
```

**Verify:** `nats request "mcp.server.${SERVER_ID}.tools.list" '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' --timeout 15s`

**Discovery announcement** (same as Option A):

```bash
nats pub "mcp.control.discovery.register.${SERVER_ID}" \
  '{"server_id":"'"${SERVER_ID}"'","transport":"nats-native","queue_group":"mcp-server-'"${SERVER_ID}"'"}'
```

---

## Step 3 — Register the server in the agent registry

Registration is a **two-part** control-plane write: (1) backend target metadata in `mcp-gateway-config`, and (2) agent manifest updates in Git for callers and (optionally) the bridge workload. The agent registry is Git → controller → KV; operators validate with `agctl` before merge.

See [registry-operations.md](registry-operations.md) for the full pipeline and [registry.md](registry.md) for field definitions.

### 3.1 Register the backend target (gateway routing)

Write a backend entry keyed under `mcp-gateway-config` (**proposed** key shape from [bootstrap-day-zero § Step b.2](bootstrap-day-zero.md#b2-mcp-backend-server-for-gateway-routing)):

```bash
cat > /tmp/backend-github.json <<EOF
{
  "server_id": "${SERVER_ID}",
  "tenant": "${TENANT}",
  "enabled": false,
  "transport": "stdio-sidecar",
  "trust_domain": "${TRUST_DOMAIN}",
  "owner_team": "platform-sre",
  "lifecycle_state": "provisioning"
}
EOF

nats kv put mcp-gateway-config "backend/${SERVER_ID}" @/tmp/backend-github.json
```

| Field | Required | Notes |
|---|---|---|
| `server_id` | yes | Must match Step 1 slug |
| `tenant` | yes (soft tenancy) | Omit when NATS account is the tenant boundary |
| `enabled` | yes | **`false` until Step 6** — gateway must not forward production traffic |
| `transport` | yes | `stdio-sidecar` \| `nats-native` \| `http-bridge` |
| `owner_team` | yes | Escalation contact |
| `lifecycle_state` | yes | Use `provisioning` until Step 6 promotes to `active` |

**Forbidden at this step:**

- Do **not** set `enabled: true` until Step 6.
- Do **not** set `lifecycle_state: "active"` on agent manifests until Step 6 (below).
- Do **not** hand-write `mcp-agent-registry` KV except break-glass ([registry-operations § ACL rule](registry-operations.md#roles)).

### 3.2 Update agent manifests (callers + bridge)

Add the backend audience to every agent that will invoke this server. Edit the signed TOML under your manifest repo:

```bash
# File: agents/acme/smoke-agent.toml
```

```toml
# agents/acme/smoke-agent.toml — extend oncall-agent.toml pattern
# (rsworkspace/crates/trogon-agent-registry/examples/oncall-agent.toml)
[record]
agent_id = "acme/smoke-agent"
agent_version = "1.0.1"
# ... owner_team, allowed_workloads, allowed_tools ...
allowed_audiences = [
  "urn:trogon:mcp:gateway:acme:gw-prod-1",
  "urn:trogon:mcp:backend:acme:github",
  "mcp.server.github",
]
allowed_purposes = ["integration.smoke"]
lifecycle_state = "deprecated"   # do NOT set "active" until Step 6
# [signature] block required — see examples/
```

If the sidecar presents an SVID, add a bridge agent manifest with matching `allowed_workloads` and the same `lifecycle_state = "deprecated"` until Step 6.

**Required manifest fields:** see [registry.md § Field definitions](registry.md#field-definitions). For MCP integration you must include `allowed_audiences` with `urn:trogon:mcp:backend:{tenant}:{server_id}` and keep `lifecycle_state` **`deprecated` until Step 6**.

**Validate locally before PR:**

```bash
cd /path/to/agent-manifest-repo

agctl registry sync \
  --repo . \
  --agents-dir agents \
  --verify-key "${REGISTRY_VERIFY_KEY}"
```

Expected output:

```text
ok: validated N manifest(s) at git commit <sha>
```

Merge via PR per [registry-operations § Normal change workflow](registry-operations.md#normal-change-workflow). Controller projects to KV within ~30 s.

```bash
nats request mcp.registry.agent.lookup '{"agent_id":"'"${AGENT_ID}"'"}' --timeout 5s | jq .
# Expect "lifecycle_state":"deprecated" until Step 6
```

---

## Step 4 — Publish a minimal policy

Publish a Tier-1 / CEL policy bundle that **allows one tool** on your server and **denies** (or delegates deny to SpiceDB for) everything else. Phase 1 today runs a hardcoded CEL gate plus SpiceDB on gated methods; bundle-driven rules are loaded from **`mcp-gateway-config`** when the bundle loader is active (**proposed** hot-reload — see [bootstrap-day-zero § Step c](bootstrap-day-zero.md#step-c--publish-minimum-viable-policy-bundle)).

### 4.1 Author the bundle

Create `bundles/acme-github-smoke.yaml`:

```yaml
# Tier-1 declarative rules
apiVersion: trogon.ai/mcp-gateway-config/v1
kind: McpGatewayBundle
metadata:
  name: acme-github-smoke
  tenant: acme
  revision: 1

# Default posture: deny gated mutations unless a rule matches
default_decision: deny

rules:
  - name: allow-github-search-only
    when:
      subject: "mcp.gateway.request.github.tools.call"
    cel: |
      mcp.method == "tools/call"
      && mcp.server_id == "github"
      && mcp.tool.name == "search_repositories"
      && jwt.sub == "smoke-tester@acme.example"
    decision: allow

  - name: deny-all-other-github-tools
    when:
      subject: "mcp.gateway.request.github.tools.call"
    cel: |
      mcp.method == "tools/call" && mcp.server_id == "github"
    decision: deny
    reason: "tool not in allowlist for acme/github integration"

```

Create manifest pointer `bundles/acme-github-smoke-manifest.json`:

```json
{
  "bundle_name": "acme-github-smoke",
  "revision": 1,
  "tenant": "acme",
  "artifact_sha256": "REPLACE_WITH_DIGEST",
  "signed_at": "2026-05-28T00:00:00Z"
}
```

Sign with org NKey in production (Phase 3). Dev clusters may load unsigned bundles only when bootstrap posture allows ([bootstrap-day-zero § 2.5](bootstrap-day-zero.md#25-mode-overrides-operator-intent)).

### 4.2 Seed SpiceDB tuples (Phase 1 PDP)

Phase 1 **`tools/call`** checks SpiceDB even when CEL allows ([trogon-mcp-gateway README](../../rsworkspace/crates/trogon-mcp-gateway/README.md#spicedb-gated-toolscall-and-resourcesread)). Seed the smoke tuple:

```bash
export SPICEDB_ENDPOINT=localhost:50051
export SPICEDB_TOKEN=dev-token

zed relationship create \
  trogon/mcp_tool:github\|search_repositories#allowed@trogon/principal:smoke-tester@acme.example

# Confirm deny path — no tuple for delete_repository
zed permission check \
  trogon/mcp_tool:github\|delete_repository call \
  trogon/principal:smoke-tester@acme.example
```

Expect `PERMISSIONSHIP_NO_PERMISSION` for the delete check.

### 4.3 Publish to KV

```bash
nats kv put mcp-gateway-config "bundle/acme-github-smoke/rev/1" @bundles/acme-github-smoke.yaml
nats kv put mcp-gateway-config "bundle/active" @bundles/acme-github-smoke-manifest.json

# Optional belt-and-braces reload signal (proposed control subject)
proposed: nats pub mcp.control.bundle.reload '{"tenant":"acme","revision":1}'
```

**How the gateway picks up policy:**

| Mechanism | Detail |
|---|---|
| **KV bucket** | `mcp-gateway-config` |
| **Active pointer key** | `bundle/active` |
| **Watch path** | Gateway replicas watch `mcp-gateway-config` ([operator overview § JetStream KV](mcp-gateway-operator-overview.md#43-jetstream-kv-throughput)) |
| **Propagation latency** | Target **< 5 s P99** after KV revision; allow up to **30 s** after first deploy before smoke tests |
| **Phase 1 fallback** | With endpoint unset, SpiceDB gate uses in-process hardcoded CEL; tuples still required when `MCP_GATEWAY_SPICEDB_ENDPOINT` is set |

**Verify bundle visibility:**

```bash
nats kv get mcp-gateway-config bundle/active
# Gateway logs (kubectl logs / journalctl): search for "bundle loaded" or readiness check
```

---

## Step 5 — Configure the trust bundle (SVID servers only)

**Skip this step** when:

- The third-party MCP is stdio-only with no workload attestation, and
- The sidecar does not present an `actor_token` SVID to STS, and
- Upstream GitHub/API credentials live only in the sidecar environment.

**Rationale:** Trust bundles anchor SPIFFE ID verification for STS exchange ([integration touch-points § Trust bundles](integration-touchpoints.md#7-gateway--trust-bundle-distribution)). Without SVID presentation, STS derives `wkl` from other attestation paths or dev sentinels; loading a bundle adds no value.

**Perform this step** when the bridge or native server presents an X.509-SVID or JWT-SVID (e.g. SPIRE agent socket).

### 5.1 Publish PEM to `mcp-trust-bundles`

```bash
export MCP_STS_TRUST_BUNDLE_PATH=/path/to/acme.local.bundle.pem
export MCP_STS_TRUST_DOMAIN=acme.local

cargo run -p trogon-sts --bin trogon-sts-publish-trust-bundle
```

Equivalent:

```bash
nats kv put mcp-trust-bundles acme.local @/path/to/acme.local.bundle.pem
```

KV key = **trust domain** (not SPIFFE path). STS watches this bucket ([`TRUST_BUNDLES_KV_BUCKET`](../../rsworkspace/crates/trogon-sts)).

### 5.2 Verify

```bash
nats kv get mcp-trust-bundles acme.local | head -3
# Expect: -----BEGIN CERTIFICATE-----
```

Attempt STS exchange with a valid SVID for `spiffe://acme.local/ns/mcp/sa/github-bridge`. Missing bundle → fail-closed ([failure-mode matrix row 15](failure-mode-matrix.md#failure-mode-matrix)).

---

## Step 6 — Promote to active

Flip lifecycle flags only after Steps 2–4 succeed and (if applicable) Step 5 is verified.

### 6.1 Promote backend target

```bash
cat > /tmp/backend-github-active.json <<EOF
{
  "server_id": "${SERVER_ID}",
  "tenant": "${TENANT}",
  "enabled": true,
  "transport": "stdio-sidecar",
  "trust_domain": "${TRUST_DOMAIN}",
  "owner_team": "platform-sre",
  "lifecycle_state": "active"
}
EOF

nats kv put mcp-gateway-config "backend/${SERVER_ID}" @/tmp/backend-github-active.json
```

### 6.2 Promote agent manifest(s)

Set `lifecycle_state = "active"` in `agents/acme/smoke-agent.toml` (and bridge agent if used). Bump `agent_version`. Re-sign, validate, merge:

```bash
agctl registry sync --repo . --verify-key "${REGISTRY_VERIFY_KEY}"
git commit -am "promote ${SERVER_ID} integration agents to active"
# merge PR → controller sync
```

Confirm audit:

```bash
nats sub "mcp.audit.registry.registered" --count=1
# or mcp.audit.registry.bumped after version bump
```

### 6.3 Mint mesh token

Exchange bootstrap JWT on STS ([sts-exchange.md](sts-exchange.md)):

```bash
nats request mcp.sts.exchange "$(cat <<EOF
{
  "subject_token": "${BOOTSTRAP_USER_JWT}",
  "subject_token_type": "urn:ietf:params:oauth:token-type:jwt",
  "actor_token": "${SVID_OR_DEV_SENTINEL}",
  "audience": "urn:trogon:mcp:backend:${TENANT}:${SERVER_ID}",
  "purpose": "integration.smoke",
  "agent_id": "${AGENT_ID}"
}
EOF
)" --timeout 5s > /tmp/sts-response.json

export MESH_TOKEN=$(jq -r '.access_token' /tmp/sts-response.json)
```

### 6.4 Subscribe to audit + send `tools/list` via SDK path

Terminal A — audit tail:

```bash
nats sub "mcp.audit.>" --queue audit-smoke
```

Terminal B — `tools/list` through the gateway edge zone. The supported client path is `trogon-a2a-sdk` ([sdk.md](sdk.md)); wire the SDK per [trogon-a2a-sdk README](../../rsworkspace/crates/trogon-a2a-sdk/README.md) and call `call_mcp(server_id, tools_list, …)` per the SDK contract. Interim NATS request (same path the SDK uses):

```bash
nats request "mcp.gateway.request.${SERVER_ID}.tools.list" \
  '{"jsonrpc":"2.0","id":42,"method":"tools/list","params":{}}' \
  --header "Authorization: Bearer ${MESH_TOKEN}" --timeout 15s | jq .
```

Expect JSON-RPC success, audit on `mcp.audit.allow.request.tools`, and registry lookup showing `lifecycle_state: "active"`.

---

## Step 7 — Smoke-test a denied call

Prove policy enforcement by invoking a **denied** tool.

### 7.1 Call denied tool

```bash
nats request "mcp.gateway.request.${SERVER_ID}.tools.call" \
  "$(cat <<EOF
{
  "jsonrpc": "2.0",
  "id": 99,
  "method": "tools/call",
  "params": {
    "name": "delete_repository",
    "arguments": { "owner": "acme", "repo": "sandbox" }
  }
}
EOF
)" \
  --header "Authorization: Bearer ${MESH_TOKEN}" \
  --timeout 15s | jq .
```

### 7.2 Expected JSON-RPC error

SpiceDB deny (no tuple) or explicit bundle deny:

```json
{
  "jsonrpc": "2.0",
  "id": 99,
  "error": {
    "code": -32100,
    "message": "policy_deny",
    "data": {
      "server_id": "github",
      "tool": "delete_repository"
    }
  }
}
```

Code `-32100` = `POLICY_DENY` ([failure-mode matrix § JSON-RPC quick reference](failure-mode-matrix.md#json-rpc-code-quick-reference)). If SpiceDB is unreachable, expect `-32107` instead — that indicates infra failure, not policy ([row 1](failure-mode-matrix.md#failure-mode-matrix)).

### 7.3 Expected audit envelope

Subject `mcp.audit.deny.request.tools` with `outcome: "deny"`, `jsonrpc_method: "tools/call"`, caller identity fields, and no backend publish ([audit envelope](../../rsworkspace/crates/trogon-mcp-gateway/src/audit.rs)). Confirm the sidecar never received `delete_repository`.

---

## Step 8 — Roll back / decommission

**Soft deprecation:** set agent `lifecycle_state = "deprecated"` in Git, set backend `enabled: false` in KV, drain in-flight traffic.

**Hard revocation:** set `lifecycle_state = "revoked"`, stop bridge replicas, remove SpiceDB tuples — see [registry-operations § Emergency revocation](registry-operations.md#emergency-revocation).

**KV cleanup:** delete `mcp-gateway-config/backend/{server_id}`; repoint `bundle/active`; revert agent manifests in Git (controller syncs KV). Do not hand-delete `mcp-agent-registry` except break-glass.

---

## Troubleshooting

Five common failures during third-party MCP integration. Each row includes a one-line diagnostic; canonical behaviour is in [failure-mode-matrix.md](failure-mode-matrix.md).

| # | Symptom | One-line diagnostic | Matrix |
|---|---|---|---|
| 1 | **Registry write rejected** — CI or controller refuses manifest | `agctl registry sync --repo . --verify-key "${REGISTRY_VERIFY_KEY}" 2>&1 \| tail -5` | [FM-REGISTRY](failure-mode-matrix.md#failure-mode-matrix) row 13 / registry NACK |
| 2 | **Policy not picked up** — gated calls still allow-all or `-32108 no_policy` | `nats kv get mcp-gateway-config bundle/active && nats kv history mcp-gateway-config bundle/active` | Row 5 (bundle missing) |
| 3 | **Trust bundle mismatch** — STS `invalid_token` / SVID rejected | `nats kv get mcp-trust-bundles ${TRUST_DOMAIN} \| openssl x509 -noout -subject 2>/dev/null \|\| echo MISSING` | Row 15 |
| 4 | **Server timeout** — client sees `-32102` | `nats request "mcp.server.${SERVER_ID}.tools.list" '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' --timeout 5s` | Row 8 |
| 5 | **Audit silent** — requests succeed but no JetStream records | `nats stream info MCP_AUDIT && nats pub mcp.audit.allow.request.tools '{"probe":true}'` | Row 14 |

Common fixes: re-sign manifests / bump version (row 1); restart gateway if Phase 1 bundle watch absent (row 2); match KV trust key to SPIFFE trust domain (row 3); confirm sidecar subscription on `mcp.server.${SERVER_ID}.>` (row 4); create `MCP_AUDIT` stream or unset `MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT` (row 5). Full behaviour: [failure-mode-matrix.md](failure-mode-matrix.md).

---

*Document type: Diátaxis **how-to**. For conceptual background, read [mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md). For wire-level registry API detail, read [registry.md](registry.md).*
