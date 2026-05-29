# ADR 0028: First-Party `mcp-pack` Bundle Composition

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | First-party `mcp-pack` bundle; unblocks bundle loader acceptance tests, day-zero policy for tuple derivation and list shaping, and operator onboarding without custom bundle authoring |
| **Related** | [ADR 0010](0010-bundle-format.md), [ADR 0013](0013-hierarchical-policy-merge.md), [ADR 0015](0015-tools-list-filtering.md), [ADR 0023](0023-schema-cache-invalidation.md), ADR 0025 *(component pooling — sibling wave)*, ADR 0027 *(redaction pipeline — sibling wave)*, [howto-write-bundle.md](../identity/howto-write-bundle.md), [wasm-bundle-format.md](../identity/wasm-bundle-format.md), [mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md), [reference-audit-envelope.md](../identity/reference-audit-envelope.md), [hierarchical-policy-merge.md](../identity/hierarchical-policy-merge.md); `rsworkspace/crates/trogon-mcp-gateway/tests/bundle_load_hot_reload.rs`, `tools_list_filter.rs`, `schema_cache_invalidation.rs`, `audit_envelope_shape.rs`, `wasm_host_abi.rs`, `policy_eval.rs`, `hierarchical_policy_merge.rs` |

## Context

Phase 3 ships signed policy bundles as the unit operators load, verify, and hot-swap ([ADR 0010](0010-bundle-format.md)). Day-zero gateways that enforce policy without a tenant-authored pack currently hit `-32108 no_policy` ([ADR 0013](0013-hierarchical-policy-merge.md)) or retain Phase 1 hardcoded SpiceDB tuple logic in `policy.rs` that does not compose with hierarchical merge, list filtering ([ADR 0015](0015-tools-list-filtering.md)), or schema-cache sniff hooks ([ADR 0023](0023-schema-cache-invalidation.md)).

A **first-party `mcp-pack`** gives operators useful defaults: SpiceDB resource-tuple derivation for gated MCP methods, catalogue shaping aligned with call-time authorization, a schema-learner WASM component, and a default audit envelope template aligned with the audit envelope schema ([reference-audit-envelope.md](../identity/reference-audit-envelope.md)). Tenants **extend/override** the pack — not fork the gateway — and `mcp-pack` must remain **deletable** when a fleet supplies its own bundle.

**Author workflow alignment.** [howto-write-bundle.md](../identity/howto-write-bundle.md) documents tenant bundle layout (`manifest.toml`, `policies/*.cel`, optional `components/*.wasm`, NKey signing). `mcp-pack` is the reference implementation of that layout at org scope: same manifest schema, same signing gate, same hierarchical merge slot — with **no customer-specific JWT claims, tenant slugs, or server ids** baked into rules. Tenant overlays carry customer fields per [hierarchical-policy-merge.md](../identity/hierarchical-policy-merge.md).

**What breaks if this stays undecided**

| Gap | Impact |
|-----|--------|
| Block F item 6 | No acceptance fixture for bundle loader, Wasmtime linker, or `agctl mcp bundle validate` |
| Block E list/call parity | Tuple derivation stays env-driven in Rust instead of one CEL program shared with `tools/list` |
| Schema cache + redaction | No Tier-3 hook to emit structured sniff events beyond native gateway sniff ([ADR 0023](0023-schema-cache-invalidation.md)) |
| Operator docs | How-to assumes a working example pack; without `mcp-pack`, examples are purely illustrative |
| Signing trust roots | First-party publisher NKey custody undefined relative to tenant publisher keys ([ADR 0010](0010-bundle-format.md)) |

**Current branch snapshot**

- No `bundles/mcp-pack/` tree on disk yet; bundle loader and Wasmtime pool are scaffolds (`tests/bundle_load_hot_reload.rs`, `tests/wasm_host_abi.rs`).
- Phase 1 tuple shapes live in [trogon-mcp-gateway README](../../rsworkspace/crates/trogon-mcp-gateway/README.md) and `config.rs` constants (`DEFAULT_SPICEDB_TOOL_RESOURCE_TYPE`, `DEFAULT_SPICEDB_RESOURCE_TYPE`).
- Audit envelope sample lives in [reference-audit-envelope.md](../identity/reference-audit-envelope.md); gateway `AuditEnvelope` is a subset today (`tests/audit_envelope_shape.rs`).

---

## Decision

Ship **`mcp-pack` v0.1.0** as a signed first-party policy bundle named **`_global/mcp-pack`**, composed of Tier 2 CEL programs plus one Tier 3 WASM component, distributed **in-tree as canonical source** and **mirrored to NATS KV generation 1** for fleet hot-swap. Gateways load it at the **organisation** merge level by default; operators may disable or replace it without forking `trogon-mcp-gateway`.

### Bundle identity and layering

| Field | Value |
|-------|-------|
| Manifest `name` | `_global/mcp-pack` |
| Manifest `version` (v0.1.0) | `0.1.0` |
| `target_wit` | `trogon:mcp-policy@0.1.0` ([mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md)) |
| `min_gateway_version` | `0.1.0` *(first gateway release that activates bundles — adjust at implementation tag)* |
| Merge level | Organisation (`policy/org/mcp-pack` pointer **(proposed)** or embedded default when first-party pack enabled) |
| Tiers in v0.1.0 | Tier 2 CEL + Tier 3 WASM only (no Tier 1 YAML) |

**Layering inside the pack** (evaluation order: CEL → NATS-callout → WASM):

```
manifest.toml
policies/          # Tier 2 — tuple derivation, list filters, audit template
components/        # Tier 3 — schema-learner.wasm only in v0.1.0
schemas/           # optional JSON fixtures for CI dry-run (not authoritative for authz)
signatures/
  manifest.sig
```

### v0.1.0 contents (in scope)

#### 1. Resource-tuple derivation (CEL)

Normative programs derive SpiceDB `(resource_type, resource_id, permission, subject)` for gated methods. They **replace** hardcoded Rust tuple builders on the policy path when `mcp-pack` is active. Subject remains `trogon/principal:{jwt.sub}` with legacy fallbacks documented in README — not reimplemented in the pack.

| Stable `program.id` | `class` | Methods | Resource shape |
|---------------------|---------|---------|----------------|
| `mcp-pack/tuple-tools-call` | `ingress_gate` | `tools/call` | `trogon/mcp_tool:{server_id}\|{normalized_tool_name}` permission `call` |
| `mcp-pack/tuple-resources-read` | `ingress_gate` | `resources/read` | `trogon/mcp_resource:{normalized_uri}` permission `read` |
| `mcp-pack/tuple-prompts-get` | `ingress_gate` | `prompts/get` | `trogon/mcp_prompt:{server_id}\|{normalized_prompt_name}` permission `get` **(proposed object type — schema TBD)** |

Derivation rules:

- `server_id` from ingress subject (`nats.subject` segment after `gateway.request.`).
- Virtual MCP names: split on **first** `::` only (see [reference-virtual-mcp.md](../identity/reference-virtual-mcp.md)); native tool name is the suffix; `server_id` is the prefix segment.
- Tuple programs expose derived strings via CEL locals consumed by downstream `spicedb.check(resource, permission, subject)` calls ([howto-write-bundle.md](../identity/howto-write-bundle.md) argument order).
- Programs are **allow guards** that invoke `spicedb.check` with derived tuples; they do not embed tenant-specific allow lists.

Sketch (illustrative — not on disk):

```cel
// policies/tuple-tools-call.cel — program id mcp-pack/tuple-tools-call
mcp.method == "tools/call"
&& spicedb.check(
     "trogon/mcp_tool:" + server.id + "|" + mcp.tool.name,
     "call",
     "trogon/principal:" + jwt.sub
   )
```

`server.id` binding **(proposed)** — host injects parsed `server_id` alongside existing `mcp.*` roots (see [reference-cel-variables.md](../identity/reference-cel-variables.md)).

#### 2. Catalogue shaping defaults ([ADR 0015](0015-tools-list-filtering.md))

| Stable `program.id` | `class` | Behavior |
|---------------------|---------|----------|
| `mcp-pack/list-tools` | `tools_list_filter` | Re-evaluate merged call rule per tool; pure builtins only |
| `mcp-pack/list-resources` | `resources_list_filter` | Same for `resources/list` |
| `mcp-pack/list-prompts` | `prompts_list_filter` | Same for `prompts/list` |

Default posture: **visibility matches invoke permission** — a tool visible in `tools/list` iff the tuple program would allow `tools/call` for that principal. Implementation prefetches SpiceDB via one `BulkCheckPermission` per list ([ADR 0015](0015-tools-list-filtering.md), [bulk-check-permission.md](../identity/bulk-check-permission.md)); list filter CEL reads materialized results, not live `spicedb.check` inside the per-item loop.

Opt-out: operators disable list filtering per scope via bundle config flag `catalog.filter.enabled=false` ([ADR 0015](0015-tools-list-filtering.md)); tenant overlay may set this without replacing tuple programs.

#### 3. Schema-learner WASM component (ADR 0027 sibling scope — redaction pipeline)

| Field | Value |
|-------|-------|
| Component `id` | `schema-learner` |
| Path | `components/schema-learner.wasm` |
| `mode` | `advisory` |
| WIT world | `policy-bundle` @ `trogon:mcp-policy@0.1.0` |

The component runs on the **response path** after successful upstream `tools/list` (and optionally `resources/list` / `prompts/list` when schemas present). It inspects catalogue entries and emits **sniff events** consumed by the gateway schema cache ([ADR 0023](0023-schema-cache-invalidation.md)): `{ server_id, schema_hash, canonical_schema_bytes, source: "mcp_pack_schema_learner" }` **(proposed event shape)**. Native gateway sniff remains the fallback when the component is absent or traps; advisory mode must not deny list responses.

Pooling: one Wasmtime pool entry per `(bundle_digest, component_id)` per ADR 0025 *(sibling wave — component pooling)*.

#### 4. Default audit envelope template (Pin 7)

| Stable `program.id` | `class` | Phase |
|---------------------|---------|-------|
| `mcp-pack/audit-default` | `audit_enrich` **(proposed class)** | request + response |

On allow paths, `audit.emit` merges stable `extra` keys aligned with the audit envelope sample ([reference-audit-envelope.md](../identity/reference-audit-envelope.md)):

| Key | Source |
|-----|--------|
| `policy_bundle_name` | constant `_global/mcp-pack` |
| `policy_bundle_version` | manifest version at load |
| `policy_bundle_digest` | active digest ([ADR 0010](0010-bundle-format.md)) |
| `method`, `method_root`, `tool` / `resource` | `mcp.*` bindings |
| `pack_layer` | constant `first_party` |

The program does **not** replace gateway mandatory fields (`schema`, `trace_id`, `subject_in`, `decision`, …) — it only enriches `extra` per [reference-audit-envelope.md](../identity/reference-audit-envelope.md). Full Pin 7 parity remains gateway publisher responsibility (`tests/audit_envelope_shape.rs`).

### v0.1.0 explicitly deferred

| Item | Rationale |
|------|-----------|
| Tier 3 redactor WASM | Native JSONPath redaction ships first; redactor component lands with ADR 0027 |
| Default rate-limit CEL | Operator/env defaults suffice (see [reference-rate-defaults.md](../identity/reference-rate-defaults.md)); avoids surprising `-32105` on day zero |
| NATS-callout plugins | Block F item 7 — separate from first-party pack |
| Customer-specific claims (`jwt.<custom>` matchers) | Belongs in tenant overlay only |
| `prompts/get` SpiceDB schema | Object type `trogon/mcp_prompt` **(proposed)** — seed tuples and schema doc in follow-on identity work |
| llm-pack / regex guardrails | Separate bundle per plan §7 Guardrails |
| cosign supplementary attestation | NKey remains authoritative gate ([ADR 0010](0010-bundle-format.md)) |

### Distribution

| Path | Role |
|------|------|
| **In-tree canonical** | `bundles/mcp-pack/` at monorepo root — source for review, CI validate/sign, and dev `--bundle-path` |
| **NATS KV mirror (generation 1)** | Bucket `mcp-policy-bundles` **(proposed)**, key `_global/mcp-pack`, generation **1** row holds signed tarball bytes after first publish; active pointer in `mcp-gateway-config` **(proposed key `policy/org/mcp-pack/active_digest`)** |
| **OCI (release train)** | `ghcr.io/trogonstack/mcp-policy/_global/mcp-pack:mcp-policy@0.1.0+<sha256-12>` per [ADR 0010](0010-bundle-format.md) |

**Bootstrap default:** when `MCP_GATEWAY_FIRST_PARTY_PACK` **(proposed env, default `embedded`)** is `embedded`, gateway loads from compiled-in or adjacent `bundles/mcp-pack/` before KV watch; when `kv`, loader waits for KV generation 1; when `off`, no org-level pack is injected.

### Versioning relative to `trogon-mcp-gateway`

| Rule | Detail |
|------|--------|
| Independent semver | `mcp-pack` carries its own `manifest.version`; not forced to equal crate `Cargo.toml` version |
| Release coupling | Same git tag publishes both crate and pack; CI fails if `min_gateway_version` > crate version |
| Compatibility floor | `min_gateway_version` in manifest refuses load on older gateways |
| ABI pin | `target_wit = trogon:mcp-policy@0.1.0` until a major WIT bump ADR |
| Digest authority | Audit and rollback always pin content digest; semver is promotion alias only ([ADR 0010](0010-bundle-format.md)) |

**Compatibility matrix (v0.1.0)**

| `mcp-pack` | `trogon-mcp-gateway` | `trogon:mcp-policy` | WASM components | CEL `cel_version` |
|------------|----------------------|---------------------|-----------------|-------------------|
| `0.1.0` | `>= 0.1.0` *(bundle-capable release)* | `@0.1.0` only | `schema-learner` | `0.10` ([howto-write-bundle.md](../identity/howto-write-bundle.md)) |

Forward incompatibility: gateway refuses load when manifest declares future WIT major or unknown component id; audit `bundle_rejected` with `policy_bundle_wit_incompatible` ([ADR 0010](0010-bundle-format.md)).

### Overridability without forking the whole pack

[Hierarchical merge](0013-hierarchical-policy-merge.md) applies: org `mcp-pack` contributes at organisation level; tenant / server / method bundles at more-specific levels **override or narrow** per hybrid deny-sticky rules.

**Selective program replacement** **(proposed merge extension):**

1. Every `mcp-pack` CEL/WASM entry uses a **stable globally unique** `program.id` / `component.id` prefixed `mcp-pack/`.
2. A tenant overlay bundle lists only the programs it replaces, reusing the same `id` with higher `priority` at tenant level.
3. Merge compiler replaces matching ids from less-specific layers; unrelated `mcp-pack` programs remain in the effective bundle.
4. Config overlays (list filter enablement, audit extras) shallow-merge; more-specific layers may narrow only.

**Full replacement / deletion**

| Operator intent | Mechanism |
|-----------------|-----------|
| Disable first-party pack entirely | `MCP_GATEWAY_FIRST_PARTY_PACK=off` **(proposed)** and no org pointer to `_global/mcp-pack` |
| Replace entire org baseline | Publish custom org bundle to `policy/org/default` without loading `mcp-pack`; set first-party flag `off` |
| Air-gapped custom only | Omit KV generation 1 row; supply signed bundle via OCI or local path only |

Tenants must never be **required** to retain `mcp-pack` in the merge tree for the gateway to start when they supply a complete org or tenant bundle set ([bootstrap-day-zero.md](../identity/bootstrap-day-zero.md)).

### Signing ([ADR 0010](0010-bundle-format.md))

| Question | Decision |
|----------|----------|
| Who signs `mcp-pack`? | **TrogonStack first-party Operator NKey** — dedicated publisher key, not mesh STS signing material ([ADR 0006](0006-mesh-token-signing-keys.md)) |
| Public key distribution | Embedded in gateway default trust anchor list `trusted_signers_first_party` **(proposed)** plus `mcp-gateway-config/trusted_signers` in production |
| Key identifier **(proposed)** | `TROGON_MCP_PACK_SIGNER_PUB` env / config field matching `[signing].nkey_pub` in manifest |
| Tenant signers | Cannot sign `_global/mcp-pack` KV overwrites unless explicitly added to break-glass multi-signer policy **(proposed — default deny)** |

CI signs with offline seed (`agctl mcp bundle sign` **(proposed)**); unsigned or wrong-signer artifacts fail closed at loader.

---

## Consequences

### Positive

- Day-zero fleets get tuple derivation, list shaping, schema sniff hooks, and audit enrichment without authoring CEL.
- `mcp-pack` demonstrates the same layout as [howto-write-bundle.md](../identity/howto-write-bundle.md) — lowers authoring errors for tenant bundles.
- Stable `program.id` values enable surgical tenant overrides per [ADR 0013](0013-hierarchical-policy-merge.md) without copying the full pack.
- In-tree source plus KV generation 1 supports git-reviewed defaults and hot-swapped fleet updates without rebuilding the gateway binary.
- Single WIT pin (`@0.1.0`) bounds Wasmtime linker and pool sizing work (ADR 0025).

### Negative

- **First-party signing key custody** — loss or leak of the Operator NKey blocks pack promotion or enables supply-chain forgery. **Mitigation:** HSM-backed seed, dual-control rotation, overlap window with two trusted pub keys in config ([ADR 0010](0010-bundle-format.md)).
- **Org-wide blast radius** — a bad `mcp-pack` release affects every tenant until rollback. **Mitigation:** digest-pinned rollback pointer; canary gateway instances; `MCP_GATEWAY_FIRST_PARTY_PACK=off` break-glass.
- **Dual sniff paths** — native gateway sniff plus WASM learner may duplicate cache writes. **Mitigation:** idempotent `(server_id, schema_hash)` puts ([ADR 0023](0023-schema-cache-invalidation.md)); advisory WASM cannot deny traffic.
- **`trogon/mcp_prompt` not yet in schema docs** — `prompts/get` tuple program may fail SpiceDB checks until schema lands. **Mitigation:** defer enabling that program until schema published; feature flag `mcp_pack.prompts_get.enabled` **(proposed)** default false in v0.1.0.

### Neutral

- `mcp-pack` semver diverges from crate patch versions — operators track two numbers on release notes.
- OCI and KV mirrors must stay digest-consistent when both paths are used ([ADR 0010](0010-bundle-format.md)).
- v0.1.0 excludes redactor WASM; tenants add redaction via native JSONPath rules or future pack minor.

---

## Rejected alternatives

### Empty first-party pack (manifest only, no programs)

| | |
|--|--|
| **Pros** | Smallest artifact; no opinionated SpiceDB tuples. |
| **Cons** | Does not satisfy Block F item 6; day-zero `-32108` persists; how-to lacks runnable reference. |
| **Verdict** | **Rejected.** |

### Embed tuple derivation only in Rust, bundle for list/WASM extras

| | |
|--|--|
| **Pros** | Fewer CEL programs; reuses existing `config.rs` constants. |
| **Cons** | List/call drift — ADR 0015 requires one merged rule; Rust and CEL would diverge; operators cannot override tuples without code change. |
| **Verdict** | **Rejected.** Tuple derivation must live in CEL inside `mcp-pack`. |

### Publish only to OCI (no in-tree `bundles/mcp-pack/`)

| | |
|--|--|
| **Pros** | Single distribution path; familiar registry ops. |
| **Cons** | Air-gapped dev and CI offline validate require registry pull; review diffs detached from gateway repo. |
| **Verdict** | **Rejected** as sole path. In-tree canonical + OCI/KV mirror is adopted. |

### Force-load `mcp-pack` (non-deletable)

| | |
|--|--|
| **Pros** | Guarantees baseline policy on every gateway. |
| **Cons** | Violates operator requirement; regulated tenants cannot run bespoke-only policy; merge testing harder. |
| **Verdict** | **Rejected.** `MCP_GATEWAY_FIRST_PARTY_PACK=off` and custom org bundles must be supported. |

### Tenant-signed first-party pack

| | |
|--|--|
| **Pros** | Tenants could fork and sign their own copy of defaults. |
| **Cons** | Blurs trust boundary; `_global` namespace becomes meaningless; SIEM cannot distinguish TrogonStack baseline from tenant artifact. |
| **Verdict** | **Rejected** for `_global/mcp-pack`. Tenants sign **overlay** bundles under `{tenant}/…` only. |

---

## Open questions

1. **`server.id` CEL binding** — Pin in [reference-cel-variables.md](../identity/reference-cel-variables.md) Block H or derive solely from `nats.subject` parsing in CEL? Until pinned, tuple programs use `nats.subject` segment extraction **(proposed helper `nats.server_id()` )**.
2. **`trogon/mcp_prompt` SpiceDB definition** — Required before enabling `mcp-pack/tuple-prompts-get` in production; coordinate with identity schema doc (future ADR topic).
3. **Sniff event transport** — Does schema-learner call host `audit.emit`, a dedicated `schema.sniff` import **(proposed)**, or write via native cache API only? Depends on ADR 0027 host ABI finalization.
4. **Program-id override merge** — Implement in `trogon-policy-core` as explicit registry merge or document as manifest-only convention until merge crate lands ([ADR 0013](0013-hierarchical-policy-merge.md) implementation notes).
5. **Embedded vs filesystem default** — Should release binaries embed tarball bytes (`include_bytes!`) or require adjacent `bundles/mcp-pack/` mount? Affects container image size and hot-swap without image rebuild.
6. **Generation 1 KV semantics** — Confirm JetStream KV generation pinning for rollback-to-gen-1 versus latest; operator runbook defers to [wasm-bundle-format.md](../identity/wasm-bundle-format.md) §8.
7. **Gateway crate version at first ship** — `Cargo.toml` currently `0.0.1`; reconcile `min_gateway_version` when bundle loader merges (implementation tag).

---

## Rollback plan

| Scenario | Action |
|----------|--------|
| Bad `mcp-pack` digest promoted | Repoint `mcp-gateway-config` org active pointer to previous digest ([ADR 0010](0010-bundle-format.md)); gateway retains last-good pool until drain |
| WASM learner traps | Set component `mode` to disabled via hot-swapped manifest minor **(proposed)** or republish prior digest without component; native sniff continues ([ADR 0023](0023-schema-cache-invalidation.md)) |
| Operator rejects all first-party policy | `MCP_GATEWAY_FIRST_PARTY_PACK=off` **(proposed)**; remove org pointer; load tenant/org custom bundle only |
| Signing key compromise | Rotate Operator NKey; publish new pack signed with new key; update `trusted_signers_first_party`; revoke old pub key; audit `bundle_rejected` for old signer |
| WIT breaking change | Ship `mcp-pack` `0.2.0` with new major; gateway loads side-by-side pools only after host upgrade ADR |

Rollback never mutates published artifacts in place — forward fix is new semver + digest ([howto-write-bundle.md](../identity/howto-write-bundle.md) § Rollback).

---

## Implementation notes

| Surface | Change |
|---------|--------|
| `bundles/mcp-pack/` **(new tree)** | `manifest.toml`, `policies/*.cel`, `components/schema-learner.wasm`, `signatures/manifest.sig`, `README.md` |
| `rsworkspace/crates/trogon-mcp-gateway/src/bundle/` **(proposed module)** | First-party bootstrap loader; embedded path resolution; org-level default pointer |
| `rsworkspace/crates/trogon-mcp-gateway/src/policy.rs` | Delegate tuple derivation to merged CEL when pack active |
| `rsworkspace/crates/trogon-mcp-gateway/src/gateway.rs` | Invoke schema-learner on list response path; wire list filters |
| `rsworkspace/crates/trogon-mcp-gateway/src/audit.rs` | Merge `audit_enrich` extras; Pin 7 fields |
| `rsworkspace/crates/trogon-mcp-gateway/src/schema_cache/` | Consume sniff events from learner + native sniff |
| CI / release | `agctl mcp bundle validate` **(proposed)** on `bundles/mcp-pack/`; sign with Operator NKey; push OCI + KV gen 1 |
| `mcp-gateway-config` KV | `policy/org/mcp-pack/active_digest` **(proposed)**; `trusted_signers_first_party` **(proposed)** |

**Acceptance tests (existing scaffolds)**

| Test file | Coverage |
|-----------|----------|
| `tests/bundle_load_hot_reload.rs` | Load `_global/mcp-pack` fixture; hot-swap; NKey verify; `bundle_loaded` audit |
| `tests/policy_eval.rs` | Tuple derivation CEL for `tools/call` / `resources/read` |
| `tests/tools_list_filter.rs` | `mcp-pack/list-*` programs; filtered_count audit |
| `tests/schema_cache_invalidation.rs` | Sniff from list + learner event idempotency |
| `tests/audit_envelope_shape.rs` | Pin 7 fields + `extra` from `mcp-pack/audit-default` |
| `tests/wasm_host_abi.rs` | `schema-learner` host imports; advisory mode |
| `tests/hierarchical_policy_merge.rs` | Tenant overlay replaces single `mcp-pack/tuple-tools-call` id |

**Authoring checklist (aligns with how-to Steps 1–7)**

1. Create layout per [howto-write-bundle.md](../identity/howto-write-bundle.md) Step 1 — add `components/` for WASM.
2. Manifest declares all `capabilities.imports` and stable `programs[].id` values.
3. Validate offline before sign; dry-run fixtures under `bundles/mcp-pack/fixtures/` **(proposed)**.
4. Sign with Operator NKey; publish digest to KV gen 1 and OCI on release tag.

---

*Contract sources: [ADR 0010](0010-bundle-format.md); [ADR 0013](0013-hierarchical-policy-merge.md); [ADR 0015](0015-tools-list-filtering.md); [ADR 0023](0023-schema-cache-invalidation.md); [howto-write-bundle.md](../identity/howto-write-bundle.md); [wasm-bundle-format.md](../identity/wasm-bundle-format.md); [mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md); [reference-audit-envelope.md](../identity/reference-audit-envelope.md); [trogon-mcp-gateway README](../../rsworkspace/crates/trogon-mcp-gateway/README.md); `rsworkspace/crates/trogon-mcp-gateway/tests/bundle_load_hot_reload.rs`, `tools_list_filter.rs`, `schema_cache_invalidation.rs`, `audit_envelope_shape.rs`, `wasm_host_abi.rs`.*
