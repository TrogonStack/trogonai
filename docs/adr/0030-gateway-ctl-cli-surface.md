# ADR 0030: `trogon-gateway-ctl` CLI Surface

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | `MCP_GATEWAY_PLAN.md` Block G item 2 (CLI: inspect config, trace requests, validate bundles offline, dry-run policy against fixture traffic); unblocks admin HTTP handler implementation guided by `tests/admin_api.rs`, bundle author workflows in `docs/identity/howto-write-bundle.md`, and operator cache introspection tied to ADR 0023 / ADR 0014 |
| **Related** | ADR 0010 (bundle format + offline NKey verification), ADR 0013 (hierarchical policy merge — effective config view), ADR 0014 (ZedToken cache stats), ADR 0023 (schema cache stats), `docs/identity/howto-write-bundle.md`, `rsworkspace/crates/trogon-mcp-gateway/tests/admin_api.rs`, `rsworkspace/crates/trogon-mcp-gateway/src/trace.rs` (`TraceStore`, `DecisionTrace`), `rsworkspace/crates/agctl/src/mcp/mod.rs` (precedent CLI patterns) |

## Context

Block G item 2 in `MCP_GATEWAY_PLAN.md` calls for an operator CLI — **`trogon-gateway-ctl`** — that inspects live gateway state, replays policy decisions offline, validates signed bundles before promotion, and rolls up dependency health for CI and runbooks. Phase 1 shipped an in-process `trace::TraceStore` keyed by JSON-RPC request id (`rsworkspace/crates/trogon-mcp-gateway/src/trace.rs`); Phase 2 adds an admin HTTP listener (`admin_api_enabled`, default off) whose acceptance contract is already pinned in `rsworkspace/crates/trogon-mcp-gateway/tests/admin_api.rs`.

Without a stable noun/verb taxonomy now, three downstream surfaces diverge:

| Surface | Risk if CLI taxonomy is unpinned |
|---------|----------------------------------|
| Admin HTTP routes (`/admin/*`) | Handlers grow ad hoc query params; `policy explain` and `policy inspect` fork. |
| Identity how-to docs | `howto-write-bundle.md` references **proposed** `agctl mcp bundle validate`; operators lack one canonical binary name. |
| CI pipelines | Exit codes and `--output json` envelopes differ per command; bundle promotion gates cannot assert stable contracts. |

The integration test scaffold (`admin_api.rs`) anticipates four remote operations today: `POST /admin/reload`, `POST /admin/bundle/rollback`, `GET /admin/status`, and `GET /admin/policy/inspect`. The prompt additionally requires offline bundle validation, JSONL dry-run fixtures, trace lookup by JSON-RPC id, cache size introspection, and health probes — some local, some remote.

**Current branch snapshot**

- `trogon-mcp-gateway` uses `clap` for its binary args (`src/config.rs`); workspace pins `clap = 4.6.1` with `derive` (`rsworkspace/Cargo.toml`).
- `agctl mcp` exposes `health`, `servers`, `policies`, `audit` subcommands; only `health` returns success — others exit `2` with `not yet implemented` (`agctl/src/mcp/mod.rs`). Output formats today: `text` and `json` only.
- No `trogon-gateway-ctl` crate or binary exists on disk.
- `TraceStore::get(&str) -> Option<DecisionTrace>` is the only trace read API in-tree.

This ADR pins the **v0.1.0 command tree**, output contract, authentication path, and exit codes. It does not implement code.

---

## Decision

**Ship `trogon-gateway-ctl` as a dedicated workspace binary crate** **(proposed)** (`rsworkspace/crates/trogon-gateway-ctl`) with six top-level command groups — `inspect`, `trace`, `bundle`, `policy`, `cache`, `health` — parsed via **`clap` 4.x derive** (same workspace pin as `trogon-mcp-gateway` and `agctl`). All commands are **non-interactive** (no prompts, no TTY-only flows). Global flags apply to every subcommand unless noted.

### Global flags (all groups)

| Flag | Env fallback | Default | Purpose |
|------|--------------|---------|---------|
| `--output <table\|json\|yaml>` | — | `table` | Structured output mode (see Output formats). |
| `--gateway-admin-url <url>` | `MCP_GATEWAY_CTL_ADMIN_URL` **(proposed)** | `http://127.0.0.1:8081` | Admin HTTP base URL (`admin_api.rs` harness `ADMIN_LISTEN_ADDR`). |
| `--bearer-token <jwt>` | `MCP_GATEWAY_CTL_BEARER_TOKEN` **(proposed)** | *(required for remote mutating/read-admin commands)* | Bearer token for admin HTTP (`Authorization: Bearer …`). |
| `--timeout <dur>` | `MCP_GATEWAY_CTL_TIMEOUT` **(proposed)** | `30s` | HTTP client timeout. |
| `--tls-client-cert <path>` | `MCP_GATEWAY_CTL_TLS_CLIENT_CERT` **(proposed)** | — | Client certificate when `admin_require_mtls=true`. |
| `--tls-client-key <path>` | `MCP_GATEWAY_CTL_TLS_CLIENT_KEY` **(proposed)** | — | Client private key for mTLS. |
| `--nats-url <url>` | `NATS_URL` | — | Optional NATS URL for trace fallback (see Authentication). |

Human **table** mode prints columnar stdout suitable for `kubectl`-style inspection; **json** and **yaml** modes print a single envelope document to stdout (schema below).

### Command taxonomy (v0.1.0)

```text
trogon-gateway-ctl [--global flags] <group> <verb> [args]
```

| Group | Verb | Args / flags | Mode | Backend |
|-------|------|--------------|------|---------|
| **inspect** | `config` | `[--revision]` | remote | `GET /admin/config` **(proposed)** |
| **inspect** | `reload` | — | remote | `POST /admin/reload` (`admin_api.rs`) |
| **trace** | `request` | `--id <jsonrpc-id>` `[--gateway-instance <id>]` | remote / NATS fallback | `GET /admin/trace/{id}` **(proposed)** or NATS `{prefix}.gateway.admin.trace.get` **(proposed)** |
| **bundle** | `validate` | `<path>` `[--wit-check]` | **offline** | Shared bundle verifier from ADR 0010 (NKey + manifest + CEL compile; optional WIT ABI when `--wit-check`) |
| **bundle** | `dry-run` | `--bundle <path> --input <jsonl>` `[--line <n>]` `[--spicedb-endpoint <url>]` | **offline** | In-process policy evaluator **(proposed)** — no NATS publish |
| **bundle** | `rollback` | — | remote | `POST /admin/bundle/rollback` (`admin_api.rs`) |
| **policy** | `explain` | `--tool <name> --subject <subject> --method <method>` `[--session-id <id>]` `[--tenant <id>]` | remote | `GET /admin/policy/inspect` (`admin_api.rs` `POLICY_INSPECT_PATH`) |
| **cache** | `stats` | — | remote | `GET /admin/cache/stats` **(proposed)** |
| **health** | *(none — group is the verb)* | `[--probe-only]` | remote + local probes | `GET /healthz`, `GET /readyz` on operational listener **(proposed `:8080`)** + dependency rollup from `GET /admin/status` when admin enabled |

**Naming rules**

- Groups are **nouns** (`inspect`, `bundle`, `cache`); verbs are **imperative** (`config`, `validate`, `explain`).
- `health` is invoked as `trogon-gateway-ctl health` with no subcommand — the group name is the operator entry point.
- Remote commands fail fast with exit code `7` when `admin_api_enabled=false` (connection refused or HTTP 404 per `admin_api.rs` feature-flag tests).
- `inspect reload` and `bundle rollback` are **mutating**; they require admin JWT role and emit `admin.{action}` audit envelopes on the gateway side (not duplicated by the CLI).

### Per-command contracts

#### `inspect config`

Returns the **effective merged configuration** the gateway evaluates on the hot path: environment overrides, KV revision pointers, active bundle digest/generation, hierarchical policy merge outcome (ADR 0013), and feature flags (`admin_api_enabled`, `schema_cache_enabled`, etc.).

JSON `data` shape **(proposed)**:

```json
{
  "config_sha256": "…",
  "bundle_sha256": "…",
  "bundle_generation": 3,
  "policy_merge_layers": ["org-base", "tenant-acme", "server-github"],
  "feature_flags": { "admin_api_enabled": true, "schema_cache_enabled": true },
  "sources": [
    { "kind": "env", "keys": ["MCP_GATEWAY_SPICEDB_ENDPOINT"] },
    { "kind": "kv", "bucket": "mcp-gateway-config", "revision": 42 }
  ]
}
```

Maps to `GET /admin/config` **(proposed)**; until that route lands, `inspect config` MAY synthesize from `GET /admin/status` fields (`config_sha256`, `bundle_sha256`) plus local env — implementation MUST converge on the dedicated route.

#### `inspect reload`

Wraps `POST /admin/reload`. Response `data` matches admin_api contract: `{ "config_sha256", "bundle_sha256", "outcome": "success"|"failure" }`.

#### `trace request --id <jsonrpc-id>`

Reads a decision trace for a completed JSON-RPC request. Primary source: gateway admin route returning `DecisionTrace` fields from `trace.rs`:

| Field | Type | Notes |
|-------|------|-------|
| `subject_in` | string | Ingress NATS subject |
| `subject_out` | string | Egress subject |
| `jsonrpc_method` | string | e.g. `tools/call` |
| `cel_requires_spicedb` | bool | |
| `spicedb_allowed` | bool \| null | |
| `tenant` | string \| null | |
| `caller_sub` | string \| null | |
| `identity_source` | string | `IdentitySource` enum wire form |

When the id is absent from the replica that served the request, exit `4` (not found). Queue-group deployments MAY require `--gateway-instance` or NATS fan-out (Open questions).

#### `bundle validate <path>`

Offline validation per ADR 0010 and `howto-write-bundle.md`:

1. Parse `manifest.toml` and policy files.
2. Verify NKey signature against configured trusted public keys **(proposed `--trusted-signer` flag or env)**.
3. Compile CEL programs against pinned `cel-interpreter` version.
4. With `--wit-check`, verify WASM components target `trogon:mcp-policy@0.1.0` without executing guest code.

Success: exit `0`, envelope `data.result = "VALID"`. Structural or signature failure: exit `5`.

#### `bundle dry-run --bundle <path> --input <jsonl>`

Replays one or more synthetic `tools/call` (or other JSON-RPC) fixtures through the bundle evaluator **without** publishing to NATS. Input file is **JSON Lines**: one fixture object per line (compatible with `howto-write-bundle.md` fixture shape — `nats`, `jwt`, `jsonrpc` bindings).

Per-line output in json mode is appended to `data.results[]`:

```json
{
  "line": 1,
  "decision": "allow",
  "rules_fired": ["allow-create-issue"],
  "spicedb_checks": [],
  "redaction_applied": false,
  "trace_id": "…"
}
```

`--line <n>` restricts evaluation to a single 1-based line (CI convenience). SpiceDB calls use `--spicedb-endpoint` or env `MCP_GATEWAY_SPICEDB_ENDPOINT`; when unset, SpiceDB builtins stub to deny with explicit `"spicedb": "skipped"` in output.

#### `policy explain --tool <name>`

Operator-facing alias for admin dry-run inspect. Required query params mirror `admin_api.rs`: `subject`, `method`; `tool` required for `tools/call`, optional otherwise.

JSON `data` **(proposed)** — superset of admin_api minimum:

```json
{
  "decision": "allow",
  "rule_fired": "allow-create-issue",
  "trace_id": "tr_…",
  "cel_rule_id": "allow-create-issue",
  "spicedb_check": {
    "resource": "trogon/mcp_tool:github|create_issue",
    "permission": "call",
    "allowed": true
  },
  "redaction_ruleset": "github-create-issue-output"
}
```

Side-effect contract: no `{prefix}.audit.allow.request.*` publish (`admin_api.rs` policy_inspect module); admin audit `admin.policy.inspect` MAY still emit.

#### `cache stats`

Returns in-process cache gauges from the gateway worker handling the request:

| Cache | Fields in `data` **(proposed)** | Source ADR |
|-------|----------------------------------|------------|
| `schema_cache` | `entries`, `max_entries`, `hits_total`, `misses_total`, `invalidations_total` | ADR 0023 (`mcp_schema_cache_*` metrics) |
| `zed_token_cache` | `entries`, `max_entries`, `hits_total`, `misses_total`, `evictions_total` | ADR 0014 (`spicedb_cache_*` metrics) |
| `filtered_list_cache` | `entries`, `max_entries` | ADR 0015 / `tools-list-filtering.md` §6.3 |

#### `health`

Rollup for operators and CI:

| Check | Source | Degraded when |
|-------|--------|---------------|
| `process` | `GET /healthz` **(proposed)** | non-200 |
| `ready` | `GET /readyz` **(proposed)** | non-200 |
| `nats` | `GET /admin/status` → `nats.state` | not `connected` |
| `spicedb` | `GET /admin/status` → `spicedb.state` | not `connected` |

With `--probe-only`, skip admin status (useful when `admin_api_enabled=false` — probes on `:8080` only). Overall `data.status` is `ok`, `degraded`, or `down` (any critical probe down → `down`). Exit `0` for `ok`, `10` for `degraded`, `11` for `down`.

When admin API is disabled, `health --probe-only` remains available per `admin_api.rs` `feature_flag` module expectations.

### Output formats

| Mode | Library | stdout |
|------|---------|--------|
| `table` | `clap` + manual `tabled` **(proposed)** or fixed-width formatting | Human columns; errors on stderr |
| `json` | `serde_json` (workspace) | Single envelope (below) |
| `yaml` | `serde_yaml` **(proposed crate-local dep when binary lands)** | Same envelope as json |

**JSON/YAML top-level envelope** (all commands):

```json
{
  "api_version": "trogon.gateway.v1",
  "command": "policy.explain",
  "ok": true,
  "data": {},
  "errors": []
}
```

| Field | Rule |
|-------|------|
| `api_version` | Constant `trogon.gateway.v1` for v0.1.0 |
| `command` | Dot-separated group.verb (e.g. `bundle.validate`, `health`) |
| `ok` | `true` iff exit code is `0` (health treats `degraded` as `ok: true`) |
| `data` | Command payload; omitted keys mean null/absent |
| `errors` | Array of `{ "code": "<snake_case>", "message": "<human readable>" }`; empty on success |

Table mode prints `data` fields directly without the envelope wrapper; errors still go to stderr with matching exit codes.

### Authentication

| Command class | Auth |
|-------------|------|
| **Offline** (`bundle validate`, `bundle dry-run`) | None — local filesystem only |
| **Remote read/mutate** (all other commands) | HTTP admin API per `admin_api.rs` |

**Primary path — HTTP admin listener**

- Bearer JWT in `Authorization` header from `--bearer-token` / `MCP_GATEWAY_CTL_BEARER_TOKEN` **(proposed)**.
- JWT MUST include `admin` in roles claim (harness `ADMIN_ROLE`); missing role → HTTP 403 → CLI exit `3`.
- Missing or invalid JWT → HTTP 401 or 403 → exit `3`.
- MCP ingress JWT without admin role MUST NOT authorize admin paths (`admin_api.rs` `role_check` module).
- Optional mTLS: when gateway `admin_require_mtls=true`, client cert required before JWT evaluation → HTTP 401 → exit `3`.

**Secondary path — NATS request/reply (trace only, optional)**

When `--gateway-admin-url` is unreachable but `--nats-url` is set, `trace request` MAY fall back to:

```text
{prefix}.gateway.admin.trace.get
```

Request payload **(proposed)**: `{ "jsonrpc_id": "<id>" }`. Reply: `DecisionTrace` JSON or empty on miss. Subject namespace **`mcp.gateway.admin.>`** **(proposed)** is reserved for operator read models that must work without HTTP; v0.1.0 implements trace get only. All other remote commands remain HTTP-only in v0.1.0.

**Relationship to `agctl`**

`agctl mcp` remains the agent-registry / cross-product stub tree. **`trogon-gateway-ctl` is the canonical MCP gateway operator CLI** for Block G. Docs that reference `agctl mcp bundle validate` (`howto-write-bundle.md`) SHOULD alias to `trogon-gateway-ctl bundle validate` when this ADR is accepted (doc edit out of scope here).

### Exit codes (stable for CI)

| Code | Meaning | Typical commands |
|------|---------|------------------|
| `0` | Success | All; `health` when `ok` or `degraded` |
| `1` | Runtime error (I/O, HTTP 5xx, internal) | Any |
| `2` | Usage / clap parse error | Any |
| `3` | Authentication / authorization failure | Remote admin |
| `4` | Not found | `trace request` (unknown id), `policy explain` (unknown tool/subject) |
| `5` | Validation failure | `bundle validate` |
| `6` | Policy deny (when `--expect allow` **(proposed)** is set) | `bundle dry-run`, `policy explain` |
| `7` | Admin API unavailable (`admin_api_enabled=false` or 404) | Remote commands |
| `10` | Health degraded | `health` |
| `11` | Health down | `health` |

Codes `0`–`7` are stable for v0.1.x; `10`–`11` are health-specific. CI SHOULD use `--output json` and assert `ok` + exit code together.

---

## Consequences

### Positive

- **Stable operator UX** before admin handlers land — implementation and docs share one command tree.
- **CI gates** for bundle promotion: `bundle validate` exit `5` blocks merge; json envelope is machine-parseable.
- **Scaffold alignment** — CLI maps 1:1 to `admin_api.rs` routes plus proposed config/trace/cache endpoints.
- **Offline-first bundle workflow** — authors validate and dry-run without a running gateway (ADR 0010, `howto-write-bundle.md`).
- **Reuse of workspace `clap` patterns** — consistent with `trogon-mcp-gateway`, `agctl`, `trogon-sts`.

### Negative

- **Second CLI beside `agctl mcp`** — overlapping mental model for MCP operators.
  - **Mitigation:** ADR declares `trogon-gateway-ctl` canonical for gateway; deprecate `agctl mcp` MCP subcommands in a follow-up ADR or README note.
- **Proposed admin routes** (`/admin/config`, `/admin/trace/{id}`, `/admin/cache/stats`) are not in the test scaffold yet.
  - **Mitigation:** Extend `admin_api.rs` in the same PR that implements handlers; until then CLI documents degraded behavior via status fallback.
- **Queue-group trace lookup** may miss ids on peer replicas.
  - **Mitigation:** Document `--gateway-instance`; long-term KV export per `MCP_GATEWAY_PLAN.md` Block D trace note.
- **`serde_yaml` not yet a workspace dependency**.
  - **Mitigation:** Add only to `trogon-gateway-ctl` crate when implemented; v0.1.0 MAY ship json+table first if yaml slips.

### Neutral

- Table formatting library choice (`tabled` vs manual) is implementation detail.
- `bundle dry-run` SpiceDB live vs stub is operator-controlled via endpoint env.
- Human table column order may evolve; json/yaml envelope is the compatibility boundary.

---

## Rejected alternatives

### Extend `agctl mcp` only (no new binary)

| | |
|-|-|
| **Pros** | Single installed binary; `agctl/src/mcp/mod.rs` already exists. |
| **Cons** | `agctl` serves agent-registry and traffic tooling — mixing gateway admin commands confuses product boundaries; stub tree already returns exit `2` for most MCP commands; name does not match `MCP_GATEWAY_PLAN.md` Block G. |
| **Verdict** | **Rejected** as canonical surface. `agctl mcp health` may remain as a thin compatibility shim delegating to `trogon-gateway-ctl health`. |

### Interactive REPL or wizard prompts

| | |
|-|-|
| **Pros** | Lower learning curve for first-time bundle authors. |
| **Cons** | Violates scriptability requirement; poor CI fit; inconsistent with existing Trogon CLIs (`trogon-sts`, `mcp-nats-server`). |
| **Verdict** | **Rejected.** All inputs via flags, env, and stdin files (jsonl only). |

### NATS-only admin (no HTTP)

| | |
|-|-|
| **Pros** | Aligns with NATS-native control plane; avoids separate admin port/mTLS. |
| **Cons** | `admin_api.rs` scaffold is HTTP-first with JWT role gating; reload/rollback audit envelopes already specified as HTTP; harder to integrate with standard load-balancer health checks. |
| **Verdict** | **Rejected** as primary transport. NATS reserved for trace fallback only in v0.1.0. |

### Flat command namespace (no groups)

Example: `trogon-gateway-ctl validate-bundle`, `trogon-gateway-ctl explain-policy`.

| | |
|-|-|
| **Pros** | Fewer typing levels; grep-friendly flat `--help`. |
| **Cons** | Does not scale to Block G+ (audit tail, config reload, future xDS hooks); breaks parity with `agctl` / `trogon-gateway` grouped CLIs. |
| **Verdict** | **Rejected.** Noun/verb groups match plan intent and operator mental model. |

---

## Open questions

1. **Trace fan-out across queue-group members** — Should `trace request` broadcast to all replicas via NATS `{prefix}.gateway.admin.trace.get` until shared KV export lands (`MCP_GATEWAY_PLAN.md` Block D item on `TraceStore`)?
2. **`GET /admin/config` vs status payload split** — Exact field boundary between `inspect config` and `GET /admin/status` (in-flight gauges belong on status only).
3. **`bundle dry-run` jsonl schema versioning** — Pin fixture `$schema` URL in a follow-up identity reference doc?
4. **Filtered-list cache metrics** — Field names depend on ADR 0015 implementation; confirm before freezing `cache stats` json schema.
5. **Tenant scoping on admin routes** — Whether admin JWT carries tenant claim that filters config view (ADR 0001 hybrid tenancy).
6. **YAML dependency timing** — Ship v0.1.0 with json+table only, or block on `serde_yaml` in `trogon-gateway-ctl` crate?
7. **Future ADR: admin HTTP surface** — Consolidate proposed routes (`/admin/config`, `/admin/trace/{id}`, `/admin/cache/stats`) into a dedicated ADR if handler scope grows beyond CLI mapping.

---

## Rollback plan

| Control | Behavior |
|---------|----------|
| **Do not install binary** | Operators continue using direct HTTP curls against admin routes; no CLI dependency in runbooks. |
| **Feature flag `admin_api_enabled=false`** | Remote CLI commands exit `7`; offline `bundle validate` / `dry-run` unaffected. |
| **Revert CLI crate** | Removing `trogon-gateway-ctl` from workspace does not affect gateway runtime; admin HTTP remains independently useful. |
| **Envelope breaking change** | Bump `api_version` to `trogon.gateway.v2`; v0.1.x consumers pin CLI version in CI images. |

Paper rollback is immediate — this ADR introduces no runtime behavior.

---

## Implementation notes

### New crate layout **(proposed)**

| Path | Responsibility |
|------|----------------|
| `rsworkspace/crates/trogon-gateway-ctl/Cargo.toml` | Binary crate; deps: `clap`, `serde`, `serde_json`, `reqwest`, `trogon-mcp-gateway` bundle verifier modules (library extraction TBD) |
| `rsworkspace/crates/trogon-gateway-ctl/src/main.rs` | `Cli` root + dispatch |
| `rsworkspace/crates/trogon-gateway-ctl/src/cli.rs` | Group/verb `clap` enums |
| `rsworkspace/crates/trogon-gateway-ctl/src/output.rs` | Envelope serialization; table/json/yaml |
| `rsworkspace/crates/trogon-gateway-ctl/src/admin_client.rs` | HTTP client for `/admin/*` |
| `rsworkspace/crates/trogon-gateway-ctl/src/commands/inspect.rs` | `config`, `reload` |
| `rsworkspace/crates/trogon-gateway-ctl/src/commands/trace.rs` | `request` |
| `rsworkspace/crates/trogon-gateway-ctl/src/commands/bundle.rs` | `validate`, `dry-run`, `rollback` |
| `rsworkspace/crates/trogon-gateway-ctl/src/commands/policy.rs` | `explain` |
| `rsworkspace/crates/trogon-gateway-ctl/src/commands/cache.rs` | `stats` |
| `rsworkspace/crates/trogon-gateway-ctl/src/commands/health.rs` | `health` |

### Gateway-side surfaces (handlers the CLI calls)

| Path | Change |
|------|--------|
| `rsworkspace/crates/trogon-mcp-gateway/src/admin/` **(proposed)** | Router: reload, rollback, status, policy/inspect, config, trace, cache/stats |
| `rsworkspace/crates/trogon-mcp-gateway/tests/admin_api.rs` | Extend scaffold for config/trace/cache routes when handlers land |
| `rsworkspace/crates/trogon-mcp-gateway/src/trace.rs` | Expose read path for admin handler (`TraceStore::get`) |
| `rsworkspace/crates/trogon-mcp-gateway/src/schema_cache/` | Introspection hook for cache stats (ADR 0023) |
| `rsworkspace/crates/trogon-mcp-gateway/src/spicedb.rs` | ZedToken cache introspection (ADR 0014) |

### Acceptance tests **(proposed)**

| Test file | Pins |
|-----------|------|
| `rsworkspace/crates/trogon-gateway-ctl/tests/cli_parse.rs` | `clap` command tree + global flags |
| `rsworkspace/crates/trogon-gateway-ctl/tests/bundle_validate.rs` | Offline validate against fixture bundle from ADR 0010 |
| `rsworkspace/crates/trogon-gateway-ctl/tests/envelope_json.rs` | `--output json` top-level shape per command |
| `rsworkspace/crates/trogon-mcp-gateway/tests/admin_api.rs` | End-to-end HTTP contracts (existing scaffold) |

### Command → HTTP mapping summary

| CLI | HTTP |
|-----|------|
| `inspect config` | `GET /admin/config` **(proposed)** |
| `inspect reload` | `POST /admin/reload` |
| `bundle rollback` | `POST /admin/bundle/rollback` |
| `policy explain` | `GET /admin/policy/inspect?subject=&method=&tool=` |
| `cache stats` | `GET /admin/cache/stats` **(proposed)** |
| `health` | `GET /healthz`, `GET /readyz`, `GET /admin/status` |
| `trace request` | `GET /admin/trace/{id}` **(proposed)** |

---

*Contract sources: `MCP_GATEWAY_PLAN.md` Block G item 2, `rsworkspace/crates/trogon-mcp-gateway/tests/admin_api.rs`, `rsworkspace/crates/trogon-mcp-gateway/src/trace.rs`, `docs/identity/howto-write-bundle.md`, `docs/adr/0010-bundle-format.md`, `docs/adr/0014-bulk-check-zedtoken-cache.md`, `docs/adr/0023-schema-cache-invalidation.md`, `rsworkspace/crates/agctl/src/mcp/mod.rs`, `rsworkspace/Cargo.toml` (clap pin)*
