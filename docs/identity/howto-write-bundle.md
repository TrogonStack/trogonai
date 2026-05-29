# How to write a CEL-only policy bundle

**Diátaxis:** how-to (goal-oriented, imperative steps).

**Audience:** platform engineers and identity engineers authoring Trogon MCP gateway policy bundles for Phase 2 (CEL tier). Phase 3 adds WASM components to the same pack layout; this guide covers the CEL-only path that ships today.

**Related:** [WASM bundle format reference](wasm-bundle-format.md) · [Policy DSL choice (CEL)](policy-dsl-choice.md) · [Rate limiting](rate-limiting.md) · [Hierarchical policy merge](hierarchical-policy-merge.md) · [Audit envelope reference](reference-audit-envelope.md) · [MCP gateway operator overview](mcp-gateway-operator-overview.md)

This guide walks through authoring, validating, signing, publishing, and hot-swapping a **CEL-only** policy bundle. The worked example gates a single MCP tool call (`create_issue` on server `github`) with SpiceDB authorization, a per-caller rate limit, and an audit enrichment field on success.

| Placeholder | Example value |
|---|---|
| Tenant | `acme` |
| MCP `server_id` | `github` |
| Tool name (native) | `create_issue` |
| Bundle slug | `acme/github-create-issue-gate` |
| Bundle semver | `1.0.0` |
| NATS subject prefix | `mcp` (`MCP_PREFIX`) |
| Signing NKey public key | `UAB…` (base32-encoded public half) |

Replace these with your deployment values before running commands.

---

## Goal

Author and publish a CEL-only policy bundle that allows a single MCP tool call gated by a SpiceDB check, with an audit extra field and a per-caller rate limit.

When complete, the gateway loads your bundle from NATS KV (or OCI mirror), evaluates the CEL rule on `tools/call`, calls SpiceDB for fine-grained authorization, enforces a token-bucket limit keyed by caller, merges audit extras on allow, and hot-swaps without restart.

---

## Prerequisites

Confirm the following before Step 1. If any item is missing, complete [Bootstrap / day-zero](bootstrap-day-zero.md) or [How to integrate a third-party MCP server](howto-integrate-third-party-mcp.md) first.

1. **Gateway running with a target registered.** `trogon-mcp-gateway` is subscribed to `mcp.gateway.request.>` under queue group `mcp-gateway`. The backend target `github` (or your `server_id`) is registered in `mcp-gateway-config` and reachable on `mcp.server.github.>`.

2. **SpiceDB endpoint reachable.** `MCP_GATEWAY_SPICEDB_ENDPOINT` is set in production. Schema includes `trogon/mcp_tool`, `trogon/mcp_resource`, and `trogon/principal` ([trogon-mcp-gateway README](../../rsworkspace/crates/trogon-mcp-gateway/README.md)). Seed tuples for your smoke principal before dry-run:

   ```text
   trogon/mcp_tool:github|create_issue#caller@trogon/principal:user:alice@acme.com
   ```

   Resource id uses `{server_id}|{tool_name}` (pipe separator). Permission checked by the gateway default is `call`.

3. **NATS KV access for bundle publication.** JetStream KV bucket `mcp-gateway-config` exists ([reference-subject-grammar.md](reference-subject-grammar.md)). For binary bundle blobs, operators may also use the **proposed** bucket `mcp-policy-bundles` ([wasm-bundle-format.md §6.2](wasm-bundle-format.md#62-path-b--nats-kv-import-offline--air-gapped-fallback)).

4. **`agctl` on PATH.** Build from the workspace:

   ```bash
   cargo build -p agctl --release
   export PATH="$PWD/rsworkspace/target/release:$PATH"
   ```

   The `agctl mcp` subcommand tree exists today (`health`, `servers`, `policies`, `audit`). Bundle-specific subcommands (`bundle validate`, `bundle dry-run`, `bundle status`, `bundle rollback`) are **proposed** in Block G and documented below as the target CLI surface.

5. **Signing NKey.** Generate or reuse an operator NKey pair trusted by the gateway. Store the seed securely; you need the seed at publish time.

6. **Environment defaults for the worked example:**

   ```bash
   export NATS_URL=nats://127.0.0.1:4222
   export MCP_PREFIX=mcp
   export TENANT=acme
   export SERVER_ID=github
   export BUNDLE_SLUG=acme/github-create-issue-gate
   export BUNDLE_VERSION=1.0.0
   export NKEY_SEED=SUAM…   # never commit; CI secret
   export NKEY_PUB=UAB…     # public half for manifest.toml
   ```

---

## Step 1 — Create the bundle directory layout

A Phase 2 CEL-only bundle is a directory tree the gateway (and `agctl`) can parse offline. Unlike Phase 3 WASM packs, there is no `policy.wasm` yet — only manifest metadata and one or more `.cel` program files. Keep each rule in its own file so reviewers can diff policy changes independently of manifest bumps.

Create the layout:

```text
my-bundle/
  manifest.toml
  policies/
    allow-create-issue.cel
  README.md
```

```bash
mkdir -p my-bundle/policies
touch my-bundle/README.md
```

The `README.md` is operator-facing change notes (owner team, ticket link, rollback contact). The gateway ignores it; CI may require it for org policy.

**Phase 3 extension:** when you add WASM components, the same directory grows `policy.wasm` and `signatures/` per [wasm-bundle-format.md §3](wasm-bundle-format.md#3-oci-artifact-layout). The CEL files remain Tier 2; WASM is Tier 3 in the same pack.

---

## Step 2 — Write the manifest

The manifest declares bundle identity, compatibility pins, host capabilities your CEL programs call, and the NKey used to sign the pack. The exact on-disk filename for CEL-only packs is `manifest.toml` **(illustrative schema — loader lands Block F/E)**; Phase 3 OCI artifacts use `bundle.toml` with the same logical fields ([wasm-bundle-format.md §3.1](wasm-bundle-format.md#31-bundletoml--manifest)).

Create `my-bundle/manifest.toml`:

```toml
# Illustrative Phase 2 CEL-only manifest — verify against loader when Block F lands.

name = "acme/github-create-issue-gate"
version = "1.0.0"

# WIT pin for forward compatibility when WASM components are added later.
# CEL-only bundles still declare target_wit so hot-swap does not require manifest reshape.
target_wit = "trogon:mcp-policy@0.1.0"

# Minimum trogon-mcp-gateway semver that may activate this bundle.
min_gateway_version = "0.3.0"

# CEL interpreter pin authors wrote against ([policy-dsl-choice.md §5](policy-dsl-choice.md#51-author-declares-cel_version)).
cel_version = "0.10"

author = "platform-identity"
created_at = "2026-05-28T14:00:00Z"
description = "Allow github create_issue for authorized callers with per-caller rate limit and audit enrichment"

[capabilities]
# Host functions referenced by .cel files — gateway refuses load if undeclared import used.
imports = [
  "host.spicedb-check",
  "host.audit-emit",
  "host.rate-acquire",
]

[signing]
# Base32-encoded NKey public key — gateway verifies manifest digest signature on load.
nkey_pub = "UABKELP5N4Y4Q3Y2Z3QZ3QZ3QZ3QZ3QZ3QZ3QZ3QZ3QZ3QZ3QZ3QZ3QZ3QZ"

[[programs]]
id = "allow-create-issue"
path = "policies/allow-create-issue.cel"
class = "ingress_gate"
effect = "allow"
priority = 100
```

### Field reference (illustrative)

| Field | Required | Purpose |
|---|---|---|
| `name` | yes | Namespaced slug (`tenant/slug`). Maps to `bundle_id` in [wasm-bundle-format.md](wasm-bundle-format.md). |
| `version` | yes | Semver author version; audit only — canonical id is content digest after sign. |
| `target_wit` | yes | Host ABI pin (`trogon:mcp-policy@0.1.0`). See [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md). |
| `min_gateway_version` | yes | Refuse load if running gateway is older. |
| `capabilities.imports` | yes when builtins used | Declares `host.spicedb-check`, `host.audit-emit`, `host.rate-acquire`, etc. |
| `programs[]` | yes | Ordered list of CEL files with `id`, `path`, `class`, `effect`. |

Cross-reference [wasm-bundle-format.md §3.1](wasm-bundle-format.md#31-bundletoml--manifest) for the Phase 3 superset (`bundle_id`, cosign layers, `policy.wasm`). When both CEL and WASM ship together, one manifest covers all tiers.

---

## Step 3 — Write the CEL policy

Create `my-bundle/policies/allow-create-issue.cel`. This program is an **allow** rule evaluated on the request path for gated methods. It uses the pinned CEL variable namespace and host builtins from [policy-dsl-choice.md §4.3](policy-dsl-choice.md#43-host-builtins-vendored-cel_builtins).

Argument order for `spicedb.check` follows **`cel_builtins/`** (`resource`, `permission`, `subject`), not the plan table's `(subject, perm, resource)` ordering — see [reference-host-abi.md](reference-host-abi.md) when published.

```cel
// policies/allow-create-issue.cel
// Program class: ingress_gate (request phase)
// Effect: allow when all guards pass; implicit deny otherwise (fail closed).

// ── Request-phase match condition ───────────────────────────────────────────
// Only evaluate this rule for the one tool we intend to gate.
// mcp.method and mcp.tool.name are pinned wire roots.
(
  mcp.method == "tools/call"
  && mcp.tool.name == "create_issue"
  && nats.subject.startsWith("mcp.gateway.request.github.")
)

// ── SpiceDB authorization ───────────────────────────────────────────────────
// Resource id matches gateway normalization: {server_id}|{tool_name}
// Subject uses trogon/principal + JWT sub ([trogon-mcp-gateway README](../../rsworkspace/crates/trogon-mcp-gateway/README.md)).
&& spicedb.check(
     "trogon/mcp_tool:github|create_issue",
     "call",
     "trogon/principal:" + jwt.sub
   )

// ── Per-caller rate limit ───────────────────────────────────────────────────
// Token bucket: 5 burst, ~0.0833 tokens/sec (~5/min sustained).
// Key MUST include tenant segment to avoid cross-tenant collision ([rate-limiting.md §3](rate-limiting.md#3-dimensions)).
&& rate.acquire(
     "caller/" + jwt.tenant + "/" + jwt.sub + "/github/create_issue",
     5,
     0.0833
   )

// ── Audit enrichment on success ───────────────────────────────────────────────
// audit.emit merges into audit envelope `extra` ([reference-audit-envelope.md](reference-audit-envelope.md)).
// Category is operator-defined; keep keys stable for SIEM parsers.
&& audit.emit(
     "policy.acme.github_create_issue",
     {
       "bundle_program": "allow-create-issue",
       "change_ticket": "CHG-48291",
       "risk_tier": "standard",
       "spicedb_resource": "github|create_issue",
     }
   )
```

### Semantics notes

| Concern | Behaviour |
|---|---|
| Match scope | Rule fires only on `tools/call` + `create_issue` + `github` ingress subject. Other tools/methods fall through to other rules or default deny. |
| SpiceDB tuple | Object type `trogon/mcp_tool`, id `github\|create_issue`, permission `call`, subject `trogon/principal:{jwt.sub}`. Mismatch with schema is a common pitfall (see § Common pitfalls). |
| Audit emit | Runs only when all prior clauses are true (allow path). Denied calls do not emit this extra block. |

For variable details (`jwt.sub`, `jwt.tenant`, `mcp.tool.name`, …), see [reference-cel-variables.md](reference-cel-variables.md) **(Block H reference — forthcoming)** and the plan table cited above.

### Optional list-filter companion (not in this bundle)

If you also shape `tools/list`, add a separate program with `class = "tools_list_filter"` that **does not** call `spicedb.check`, `audit.emit`, or `rate.acquire` ([tools-list-filtering.md §4.4](tools-list-filtering.md#44-allowed-builtins-for-tools_list_filter-programs)). Keep intent aligned with the call rule to avoid catalogue drift.

---

## Step 4 — Validate offline

Run structural validation before signing. This catches manifest/schema errors, undeclared capability imports, CEL compile failures, and arity mistakes without touching a live gateway.

```bash
agctl mcp bundle validate ./my-bundle
```

**Status:** **proposed** — Block G scaffolds `agctl mcp` today (`health` only returns success; `policies list` returns `not yet implemented`). Until `bundle validate` lands, treat the command above as the contract and use the manual checklist:

Until the CLI lands, verify manually: TOML parse; program paths exist; every host call appears in `capabilities.imports`; CEL compiles against `cel-interpreter =0.10.0`; list-filter programs stay pure ([tools-list-filtering.md §4.4](tools-list-filtering.md#44-allowed-builtins-for-tools_list_filter-programs)). Success prints `result: VALID`; missing imports print `capabilities.imports omits "host.rate-acquire"` **(proposed output)**.

---

## Step 5 — Dry-run against fixture traffic

Replay a recorded JSON-RPC request through the bundle evaluator without publishing to KV. Dry-run exercises match conditions, SpiceDB stub/live mode, rate-limit math, and audit envelope shaping.

### 5.1 Create a request fixture

Save `fixtures/create-issue-allow.json`:

```json
{
  "description": "Authorized alice calls github create_issue",
  "nats": {
    "subject": "mcp.gateway.request.github.tools.call",
    "headers": {
      "traceparent": "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
      "mcp-session-id": "sess_smoke_001"
    }
  },
  "jwt": {
    "sub": "user:alice@acme.com",
    "tenant": "acme",
    "iss": "https://idp.acme.com",
    "aud": "trogon-mcp-gateway",
    "roles": ["engineer"]
  },
  "jsonrpc": {
    "jsonrpc": "2.0",
    "id": "req-001",
    "method": "tools/call",
    "params": {
      "name": "create_issue",
      "arguments": {
        "owner": "acme",
        "repo": "platform",
        "title": "Dry-run smoke issue"
      }
    }
  }
}
```

### 5.2 Run dry-run

```bash
agctl mcp bundle dry-run \
  --bundle ./my-bundle \
  --fixture ./fixtures/create-issue-allow.json \
  --spicedb-endpoint "${MCP_GATEWAY_SPICEDB_ENDPOINT:-localhost:50051}" \
  --output json
```

**Status:** **proposed** CLI. Flags mirror future gateway evaluation: inject `mcp.*`, `jwt.*`, `nats.*` bindings per [reference-cel-variables.md](reference-cel-variables.md).

Expected allow output includes `"decision": "allow"`, `"rules_fired": ["allow-create-issue"]`, populated `spicedb.checks`, `rate_limit.acquired: true`, and `audit_extra` keys from `audit.emit` **(proposed JSON shape)**. Rate-limited replays return `"decision": "deny"` with `jsonrpc_error.code: -32105`.

---

## Step 6 — Sign and publish

Signing binds the manifest digest to your NKey. Publication makes the signed artifact reachable by digest — OCI registry (production default) or NATS KV mirror (air-gapped fallback) per [wasm-bundle-format.md §6](wasm-bundle-format.md#6-distribution).

### 6.1 Compute manifest digest and sign

```bash
cd my-bundle

# Canonical manifest bytes (no trailing newline drift — use same bytes gateway verifies).
MANIFEST_DIGEST=$(openssl dgst -sha256 -binary manifest.toml | openssl base64 -A)

# Sign digest with NKey seed (illustrative — use org's nkeys tool or agctl when shipped).
nkey sign --in manifest.toml --out signatures/manifest.sig --seed "${NKEY_SEED}"
```

**Proposed:** `agctl mcp bundle sign --path ./my-bundle --seed-file "${NKEY_SEED_PATH}"`

Verify locally before publish:

```bash
nkey verify --in manifest.toml --sig signatures/manifest.sig --pub "${NKEY_PUB}"
```

Phase 3 OCI bundles add cosign layers for `bundle.toml` and `policy.wasm` ([wasm-bundle-format.md §5](wasm-bundle-format.md#5-signing)). CEL-only packs use NKey-over-manifest as the Phase 2 path.

### 6.2 Publish to OCI registry (production default)

```bash
# Tar the signed tree and push as OCI artifact (ORAS).
tar czf /tmp/acme-github-create-issue-gate-1.0.0.tar.gz \
  manifest.toml policies/ signatures/ README.md

oras push "ghcr.io/acme/mcp-policy/${BUNDLE_SLUG}:${BUNDLE_VERSION}" \
  "/tmp/acme-github-create-issue-gate-1.0.0.tar.gz:application/vnd.trogon.mcp-policy.bundle.v1+tar+gzip"

DIGEST=$(oras manifest fetch "ghcr.io/acme/mcp-policy/${BUNDLE_SLUG}:${BUNDLE_VERSION}" | jq -r .digest)
echo "Published digest: ${DIGEST}"
```

Media types are **proposed** until registered ([wasm-bundle-format.md §2.3](wasm-bundle-format.md#23-media-types-proposed)).

### 6.3 Publish to NATS KV mirror (air-gapped / fallback)

When outbound registry access is unavailable, import the same tarball into KV after offline verify:

```bash
# Proposed bucket — not yet a repo constant; see wasm-bundle-format.md §6.2.
nats kv put mcp-policy-bundles "${DIGEST}" @/tmp/acme-github-create-issue-gate-1.0.0.tar.gz
```

Update the active pointer in `mcp-gateway-config` **(proposed key layout)**:

```bash
cat > /tmp/active-pointer.json <<EOF
{
  "source": "kv",
  "digest": "${DIGEST}",
  "bundle_id": "${BUNDLE_SLUG}",
  "bundle_version": "${BUNDLE_VERSION}",
  "target_wit": "trogon:mcp-policy@0.1.0",
  "promoted_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "promoted_by": "human:alice@acme.com"
}
EOF

nats kv put mcp-gateway-config "policy/cel/active_digest" @/tmp/active-pointer.json
```

For server-scoped bundles, also update `policy/server/${SERVER_ID}/active_digest` **(proposed key)**. See [wasm-bundle-format.md §6](wasm-bundle-format.md#6-distribution) and [hierarchical-policy-merge.md §1](hierarchical-policy-merge.md#1-the-hierarchy).

---

## Step 7 — Hot-swap

The gateway watches `mcp-gateway-config` for active digest pointer changes. On update it fetches the artifact, verifies the NKey signature, compiles CEL programs, and atomically swaps the in-memory bundle revision — no process restart ([wasm-bundle-format.md §8.5](wasm-bundle-format.md#85-hot-swap-semantics)).

### 7.1 Confirm activation

```bash
agctl mcp bundle status --output json
```

**Status:** **proposed** CLI. Interim checks:

```bash
# Active pointer
nats kv get mcp-gateway-config policy/cel/active_digest

# Gateway health (implemented today)
agctl mcp health --output json

# Optional belt-and-braces control signal (proposed subject)
nats pub mcp.control.bundle.reload '{"digest":"'"${DIGEST}"'","reason":"manual promotion"}'
```

Expected status lists active `bundle_id`, `digest`, `programs_loaded`, and `ready: true` **(proposed JSON)**.

### 7.2 Live smoke test

Exchange a mesh JWT ([sts-exchange.md](sts-exchange.md)) and call the gated tool:

```bash
# Publish JSON-RPC to gateway ingress (illustrative NATS request).
nats request "mcp.gateway.request.github.tools.call" \
  "$(jq -c .jsonrpc fixtures/create-issue-allow.json)" \
  --header "Authorization: Bearer ${MESH_JWT}" \
  --timeout 10s
```

Tail audit for the enrichment fields:

```bash
agctl mcp audit tail --output json | jq 'select(.method=="tools/call" and .tool=="create_issue")'
```

**Status:** `audit tail` is scaffolded (`not yet implemented`). Interim:

```bash
nats sub "mcp.audit.>" --count 5
```

Confirm allow envelope includes `rules_fired` containing `allow-create-issue` and `extra` keys from `audit.emit` ([reference-audit-envelope.md](reference-audit-envelope.md)).

---

## Rollback

If the new bundle misbehaves after promotion, revert to the previous digest — hot-swap applies to rollback the same way as promotion. The gateway keeps the prior revision compiled until in-flight evaluations drain ([wasm-bundle-format.md §8.5](wasm-bundle-format.md#85-hot-swap-semantics)).

1. Record the current digest before promotion (`agctl mcp bundle status` or `nats kv history mcp-gateway-config policy/cel/active_digest`).
2. Repoint the active pointer to the last known-good digest:

   ```bash
   agctl mcp bundle rollback --to-digest sha256:PREVIOUS…
   ```

   **Proposed CLI.** Interim manual rollback:

   ```bash
   nats kv put mcp-gateway-config policy/cel/active_digest @/tmp/previous-active-pointer.json
   ```

3. Verify `ready: true` and re-run the Step 7 smoke test.
4. File an incident note with both digests for SIEM correlation (`policy_bundle_digest` in audit **proposed** — [wasm-bundle-format.md §4.2](wasm-bundle-format.md#42-audit-envelope-fields-proposed)).

Never mutate a published artifact in place; publish a fixed semver and roll forward when the root cause is repaired.

---

## Common pitfalls

- **Forgetting to declare capability use.** If `allow-create-issue.cel` calls `rate.acquire` but `capabilities.imports` omits `host.rate-acquire`, the loader rejects the bundle at hot-swap and the previous revision stays active ([wasm-bundle-format.md §9.2](wasm-bundle-format.md#92-manifest-missing-or-invalid)). Symptom: `-32108 no_policy` or stale policy after promotion.

- **CEL type errors that only surface at evaluation.** JSON map fields from `mcp.params` are dynamically typed. A guard like `mcp.params.owner == 123` compiles but fails at runtime when `owner` is a string — surfacing as `-32101 policy_fault`. Prefer explicit conversions or `jsonpath.get` with type checks ([reference-cel-variables.md](reference-cel-variables.md)).

- **SpiceDB resource derivation mismatched with schema.** Gateway builds `trogon/mcp_tool:{server_id}|{tool_name}`. Tuple seeds using `/` instead of `|`, or wrong server id (`github` vs virtual prefix `github::create_issue`), cause `-32100 policy_deny` despite intuitive CEL. Align seeds with [trogon-mcp-gateway README § Tuple shapes](../../rsworkspace/crates/trogon-mcp-gateway/README.md).

- **Rate-limit window too tight for burst traffic.** `rate.acquire(..., 5, 0.0833)` allows five rapid calls then throttles. Legitimate batch automations hit `-32105` without a SpiceDB deny. Raise capacity or refill, or scope the key narrower (per-agent instead of per-sub) per [rate-limiting.md §3](rate-limiting.md#3-dimensions).

- **`spicedb.check` argument order.** Implementation order is `(resource, permission, subject)`. Copy-pasting from the plan table's `(subject, perm, resource)` ordering silently checks the wrong tuple ([policy-dsl-choice.md §4.5](policy-dsl-choice.md#45-plan-namespace-vs-implementation)).

---

## Cross-references

| Document | Relevance |
|---|---|
| [wasm-bundle-format.md](wasm-bundle-format.md) | Phase 3 OCI layout, `bundle.toml`, cosign, KV import, hot-swap semantics |
| [reference-cel-variables.md](reference-cel-variables.md) | Block H reference — pinned `mcp.*`, `jwt.*`, `nats.*` roots **(forthcoming)** |
| [reference-host-abi.md](reference-host-abi.md) | Block H reference — host builtin arity and WIT mapping **(forthcoming)** |
| [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md) | `target_wit` pin and Phase 3 WASM host imports |
| [policy-dsl-choice.md](policy-dsl-choice.md) | CEL decision record, `cel_version`, builtin signatures |
| [reference-audit-envelope.md](reference-audit-envelope.md) | Audit JSON shape; `extra` merge from `audit.emit` |
| [rate-limiting.md](rate-limiting.md) | Token-bucket semantics for `rate.acquire` |
| [hierarchical-policy-merge.md](hierarchical-policy-merge.md) | Where server/tenant/org bundles merge |
| [tools-list-filtering.md](tools-list-filtering.md) | Pure predicates for `tools/list` companion programs |

---

## Appendix — Command summary

| Step | Command | Status |
|---|---|---|
| Validate | `agctl mcp bundle validate ./my-bundle` | proposed |
| Dry-run | `agctl mcp bundle dry-run --bundle ./my-bundle --fixture ./fixtures/create-issue-allow.json` | proposed |
| Sign | `agctl mcp bundle sign --path ./my-bundle --seed-file "${NKEY_SEED_PATH}"` | proposed |
| Publish (OCI) | `oras push ghcr.io/acme/mcp-policy/…` | standard ORAS |
| Publish (KV) | `nats kv put mcp-policy-bundles "${DIGEST}" @bundle.tar.gz` | proposed bucket |
| Status | `agctl mcp bundle status --output json` | proposed |
| Rollback | `agctl mcp bundle rollback --to-digest sha256:…` | proposed |
| Health (today) | `agctl mcp health --output json` | implemented |

---

*Document type: Diátaxis **how-to**. For bundle wire layout and signing theory, read [wasm-bundle-format.md](wasm-bundle-format.md). For CEL language choice and versioning, read [policy-dsl-choice.md](policy-dsl-choice.md).*
