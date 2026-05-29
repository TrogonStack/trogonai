# ADR 0010: WASM Policy Bundle Format and Signing

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-28) |
| **Date** | 2026-05-28 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | `MCP_GATEWAY_PLAN.md` Block F item 4 (bundle format + NKey signature verification), Block F item 5 (bundle loader from NATS KV with hot-swap and rollback) |
| **Related** | [wasm-bundle-format.md](../identity/wasm-bundle-format.md), [howto-write-bundle.md](../identity/howto-write-bundle.md), [mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md), [reference-host-abi.md](../identity/reference-host-abi.md), [ADR 0006](0006-mesh-token-signing-keys.md), [ADR 0001](0001-tenancy-model.md) |

## Context

Phase 2 and Phase 3 MCP gateway policy ships as **bundles**: hot-swappable artifacts that encode declarative CEL rules, optional JSON schemas, and (Phase 3) sandboxed WASM policy components. A bundle is the unit operators publish, sign, pin, roll back, and audit. Gateways load bundles without restart, compile or link guest code, and evaluate policy on every JSON-RPC request.

The design context is captured in [wasm-bundle-format.md](../identity/wasm-bundle-format.md) (on-disk layout, validation pipeline, distribution paths) and [howto-write-bundle.md](../identity/howto-write-bundle.md) (author workflow for CEL-only and WASM-extended packs). This ADR distills those specs into a single durable decision: **container format, content layout, signature scheme, addressing, and KV mirror semantics**.

### Why bundles matter

| Concern | Role of the bundle |
|---------|-------------------|
| **Policy + redaction + WASM** | One artifact carries Tier 2 CEL programs and Tier 3 WASM components under one manifest; hierarchical merge ([hierarchical-policy-merge.md](../identity/hierarchical-policy-merge.md)) selects which bundle applies per tenant. |
| **Hot-swap** | Operators promote a new digest or version pin; gateways watch KV, verify, warm pools, and flip active pointers without draining connections. |
| **Audit correlation** | Every allow/deny/challenge outcome references the activated bundle identity so SIEM and incident response can tie behavior to a signed release. |
| **Supply chain** | Tenant-authored WASM is untrusted code even inside a sandbox; **forging or tampering with a bundle is the worst possible attack surface** on the gateway control plane. Signing is non-negotiable. |

### Constraints that drove the decision

1. **Parseable without WASM execution** — CI and `agctl` must validate manifest, signatures, and size limits before Wasmtime links guest code ([wasm-bundle-format.md §1.1](../identity/wasm-bundle-format.md#11-design-goals)).
2. **Mature distribution tooling** — production fleets already run OCI registries; reusing that ecosystem beats inventing a bespoke blob store ([wasm-bundle-format.md §2.1](../identity/wasm-bundle-format.md#21-recommendation-oci-artifact)).
3. **Operational consistency with NATS** — JetStream KV is the config and trust surface for the rest of the stack (`mcp-gateway-config`, `mcp-agent-registry`, `mcp-trust-bundles` per [ADR 0001](0001-tenancy-model.md)); bundle promotion must fit the same watch-and-reload pattern.
4. **NKey lineage** — MCP gateway bootstrap and auth-callout already use NATS NKeys; bundle signing reuses operator-familiar key material rather than introducing a parallel cosign-only path for the authoritative verification gate ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block F item 4).

### What breaks if this stays undecided

- Block F Wasmtime integration cannot pin manifest schema, linker target, or verification algorithm.
- Block G `agctl mcp policies` subcommands lack a stable validate/sign/publish contract.
- Hot-swap and rollback semantics fork between "CEL-only KV blob" and "WASM OCI artifact" implementations.
- Audit envelopes cannot standardize on `policy_bundle_digest` / `bundle_loaded` / `bundle_rejected` fields ([reference-audit-envelope.md](../identity/reference-audit-envelope.md)).
- NKey signing key distribution remains ambiguous relative to [ADR 0006](0006-mesh-token-signing-keys.md) mesh-token custody.

## Decision

**Bundles are OCI artifacts** published and referenced as:

```text
mcp-policy@<semver>+<sha256-12>
```

where `<semver>` is the author-facing `version` from `manifest.toml` and `<sha256-12>` is the first twelve lowercase hex digits of the SHA-256 digest of the **canonical signed manifest bytes** (see Signing below). The OCI image manifest digest remains the authoritative immutable identifier in audit; the `mcp-policy@…` tag is the human-operator promotion alias.

**Bundle contents** (filesystem layout inside the OCI artifact layer):

| Path | Phase | Required |
|------|-------|----------|
| `manifest.toml` | 2 + 3 | yes |
| `policies/*.cel` | 2 + 3 | yes (≥1 file when CEL tier enabled) |
| `components/*.wasm` | 3 | yes when WASM tier enabled |
| `schemas/*.json` | 2 + 3 | optional |

**Signature:** the publisher NKey signs the SHA-256 digest of canonical `manifest.toml` bytes. The gateway verifies the detached signature against the **signing public key pinned in deploy config** (`mcp-gateway-config/trusted_signers` or environment bootstrap). Unsigned bundles MUST NOT activate in production.

**KV mirror:** JetStream KV bucket `mcp-policy-bundles` holds operator-published bundle bytes keyed by `{tenant}/{name}` (namespaced slug matching `manifest.toml` `name`, e.g. `acme/github-create-issue-gate`). The active version pin for each gateway instance lives in `mcp-gateway-config` and references digest + semver; KV is the air-gapped and NATS-native pull path when OCI egress is unavailable ([wasm-bundle-format.md §6.2](../identity/wasm-bundle-format.md#62-path-b--nats-kv-import-offline--air-gapped-fallback)).

**WIT target:** `trogon:mcp-policy@0.1.0`, pinned to **WASI 0.3** and the WebAssembly Component Model ([mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md), [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block F item 1). Guest components MUST export world `policy-bundle`; the host selects the Wasmtime linker from `manifest.toml` `target_wit` only.

## Consequences

### Positive

| Outcome | Detail |
|---------|--------|
| **Hot-swap without restart** | Gateways watch `mcp-gateway-config` active pointers; verified bundles compile into instance pools and flip atomically while in-flight evaluations drain on the prior digest ([wasm-bundle-format.md §8](../identity/wasm-bundle-format.md#8-loading)). |
| **Rollback via prior version pin** | Operators repoint active config to a previous OCI digest or KV-published semver; audit records `bundle_loaded` with old and new identifiers. |
| **OCI ecosystem** | Standard registries (GHCR, ECR, ACR), retention, geo-replication, and CI publish pipelines apply without custom blob infrastructure. |
| **Offline validation** | Manifest and NKey signature verify without executing guest WASM; `agctl mcp policies verify` can share the gateway loader's pre-link checks. |
| **Unified pack across tiers** | CEL-only Phase 2 bundles and Phase 3 WASM-extended bundles share one layout; authors add `components/*.wasm` without reshaping `manifest.toml`. |

### Negative

| Outcome | Detail |
|---------|--------|
| **NKey signing key distribution** | Publishers need custody of bundle signing NKeys distinct from mesh STS signing keys ([ADR 0006](0006-mesh-token-signing-keys.md)). Rotation, overlap windows, and allowed public keys in `trusted_signers` are a separate operational concern; misconfigured trust roots cause `bundle_rejected` across the fleet. |
| **Dual distribution paths** | Operators must keep OCI registry content and KV mirror bytes consistent when using both; digest mismatch between paths is a support burden. |
| **Component Model only** | Legacy WASM modules are rejected at load; authors must compile to components targeting `trogon:mcp-policy@0.1.0`. |

### Neutral

| Outcome | Detail |
|---------|--------|
| **Per-tenant bundles** | Default layout: KV key `{tenant}/{name}` and manifest `name = "{tenant}/{slug}"` scope policy to one customer ([ADR 0001](0001-tenancy-model.md) hybrid tenancy). |
| **Global bundles** | Federation-wide or org-base policy uses a reserved tenant segment (e.g. `_global/default-pack`) or org-level OCI repository; tenant override bundles take precedence per hierarchical merge rules. |
| **Optional schemas** | `schemas/*.json` surface in audit metadata only; gateway MUST NOT branch authorization on schema file contents unless explicitly referenced from CEL/WASM programs. |

## Alternatives considered

### Tarball + detached signature

| Aspect | Assessment |
|--------|------------|
| **Pros** | Simple CI output; familiar to release engineering; works air-gapped. |
| **Cons** | No standard manifest layer in registry tooling; content-addressing and promotion automation are ad hoc; harder to integrate with existing artifact pipelines. |
| **Verdict** | **Rejected** as production default. Tarballs MAY be used as CI staging input wrapped into OCI before promotion ([wasm-bundle-format.md §2.2](../identity/wasm-bundle-format.md#22-alternatives-considered)). |

### Single-file bundle (manifest + CEL + WASM concatenated)

| Aspect | Assessment |
|--------|------------|
| **Pros** | One KV write; minimal directory semantics. |
| **Cons** | Poor WASM ergonomics (cannot layer-cache WASM separately); CEL diffs opaque; violates "parse without WASM execution" when metadata is embedded opaquely. |
| **Verdict** | **Rejected.** Multi-file layout inside one OCI layer preserves reviewer-friendly diffs and selective layer uploads. |

### Unsigned bundles (dev-only trust-on-first-use)

| Aspect | Assessment |
|--------|------------|
| **Pros** | Fastest local iteration. |
| **Cons** | Catastrophic if promoted to production — any writer to KV or registry can supply arbitrary WASM policy code. |
| **Verdict** | **Rejected** for any environment where the gateway enforces policy. Dev MAY skip verify only with explicit break-glass flag documented in operator runbooks; default loader behavior is **closed**. |

### cosign / Sigstore as primary signature (spec exploratory path)

[wasm-bundle-format.md §5](../identity/wasm-bundle-format.md#5-signing) documents cosign keyless and static-key modes as an exploratory primary path. **This ADR selects NKey-over-manifest-digest** to align with MCP gateway plan Block F item 4 and existing NATS operator tooling. cosign MAY be added later as a supplementary attestation layer without changing the authoritative NKey gate.

## Implementation notes

### Manifest schema (proposed)

Normative field definitions live in [wasm-bundle-format.md §3.1](../identity/wasm-bundle-format.md#31-bundletoml--manifest) (logical superset under filename `manifest.toml` per [howto-write-bundle.md §Step 2](../identity/howto-write-bundle.md#step-2--write-the-manifest)). Minimum required fields:

| Field | Purpose |
|-------|---------|
| `name` | Namespaced slug `{tenant}/{slug}`; matches KV key suffix. |
| `version` | Semver author version; appears in `mcp-policy@<semver>+…` tag. |
| `target_wit` | Host ABI pin; must equal `trogon:mcp-policy@0.1.0` for Phase 3 MVP. |
| `min_gateway_version` | Refuse load if running gateway semver is older. |
| `author`, `created_at`, `description` | Audit and operator UI. |
| `capabilities.imports` | Declared host imports for CEL builtins and WASM guests ([reference-host-abi.md](../identity/reference-host-abi.md)). |
| `programs[]` | CEL file registry: `id`, `path`, `class`, `effect`, `priority`. |
| `[signing] nkey_pub` | Base32 public half of signing NKey embedded for operator verification; signature verified against config trust roots. |

Phase 3 additions in the same manifest:

| Field | Purpose |
|-------|---------|
| `components[]` | WASM component entries: `id`, `path` under `components/`, optional `mode` (`authoritative` \| `advisory`). |

### OCI artifact layout

```
/
├── manifest.toml
├── policies/
│   └── *.cel
├── components/          # Phase 3
│   └── *.wasm
├── schemas/             # optional
│   └── *.json
└── signatures/
    └── manifest.sig     # NKey signature over manifest.toml digest
```

Recommended OCI media types remain **proposed** in [wasm-bundle-format.md §2.3](../identity/wasm-bundle-format.md#23-media-types-proposed) until registered; loaders accept path-based detection in dev.

### Loader from KV with hot-swap

| Stage | Behavior |
|-------|----------|
| **Watch** | Gateway watches `mcp-gateway-config` keys for active bundle pointer per tenant (and optional org-base pointer). |
| **Fetch** | Pull bytes from `mcp-policy-bundles/{tenant}/{name}` at pinned semver or resolve OCI ref when `source=oci`. |
| **Verify** | Recompute manifest digest; verify NKey signature; validate manifest fields; check `target_wit` against host allowlist. |
| **Compile / pool** | Wasmtime component compile for each `components/*.wasm`; instantiate pool; call guest `init()`. |
| **Activate** | Atomic flip of in-memory active digest; drain prior pool after quiescence. |
| **Reject** | On verify failure, retain prior bundle; emit audit `bundle_rejected`; set `/ready=false` if no prior bundle. |

Control subject `mcp.control.bundle.reload` ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md)) is belt-and-braces; KV watcher is primary.

### Version pinning per gateway instance

Each gateway instance resolves:

```json
{
  "source": "oci",
  "name": "acme/github-create-issue-gate",
  "version": "1.0.0",
  "digest": "sha256:…",
  "oci_ref": "ghcr.io/acme/mcp-policy@mcp-policy@1.0.0+abc123def456",
  "target_wit": "trogon:mcp-policy@0.1.0"
}
```

Audit and rollback always pin **digest**; semver and `mcp-policy@…` tags are convenience aliases only.

### Audit outcomes

Extend gateway audit ([reference-audit-envelope.md](../identity/reference-audit-envelope.md)) with bundle lifecycle events:

| Event | When | Key fields |
|-------|------|------------|
| `bundle_loaded` | New digest activated after verify + warm | `policy_bundle_digest`, `name`, `version`, `target_wit`, `signer_nkey` |
| `bundle_rejected` | Verify or manifest validation failed | `decision_reason` (`policy_bundle_signature_invalid`, `policy_bundle_manifest_invalid`, `policy_bundle_wit_incompatible`, `policy_bundle_oversize`), `attempted_digest` |

Request-path policy evaluation repeats `policy_bundle_digest` on JSON-RPC policy errors (`-32100`, `-32101`, `-32108` per [mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md)).

### Code and config surfaces

| Surface | Change |
|---------|--------|
| `rsworkspace/crates/trogon-mcp-gateway` | Phase 3 bundle loader, NKey verify, Wasmtime pool, hot-swap |
| `agctl mcp policies` | `validate`, `sign`, `publish`, `verify`, `import`, `rollback` (Block G) |
| NATS KV | Bucket `mcp-policy-bundles`; keys `{tenant}/{name}` |
| `mcp-gateway-config` | Active pointer JSON; `trusted_signers` NKey allowlist |
| Env bootstrap | `MCP_GATEWAY_POLICY_SOURCE=oci\|kv`, trust roots path, OCI registry ref |

### Relationship to spec doc filename drift

[wasm-bundle-format.md](../identity/wasm-bundle-format.md) uses `bundle.toml` in OCI examples; [howto-write-bundle.md](../identity/howto-write-bundle.md) uses `manifest.toml` for CEL packs. **This ADR standardizes on `manifest.toml`** for all tiers. Implementations SHOULD accept `bundle.toml` as a deprecated alias for one release cycle with a loader warning.

### Signing algorithm (reference)

Verification at load time:

```text
LOAD(ref):
  artifact ← pull OCI or read KV mcp-policy-bundles/{tenant}/{name}
  REQUIRE manifest.toml present
  manifest_bytes ← canonical UTF-8 bytes (no BOM)
  manifest_digest ← SHA-256(manifest_bytes)
  REQUIRE signatures/manifest.sig present
  VERIFY nkey_sign(manifest_digest, manifest.sig, trusted_signers)
  PARSE manifest.toml → record
  VALIDATE record fields against schema
  CHECK record.signing.nkey_pub ∈ trusted_signers OR matches pinned key
  CHECK target_wit against host allowlist
  FOR EACH components/*.wasm (Phase 3):
    VERIFY component model format; size ≤ cap
  IF ok → eligible for activation
  ELSE → bundle_rejected; retain prior active digest
```

Canonical manifest bytes: UTF-8 TOML as published; loaders MUST NOT re-serialize parsed TOML for digest computation (whitespace-sensitive). Publishers SHOULD run `agctl mcp policies sign` to emit deterministic layout before OCI push.

NKey signing keys are **distinct** from mesh STS signing keys ([ADR 0006](0006-mesh-token-signing-keys.md)). Bundle keys MAY be Operator-scoped account NKeys or dedicated publisher NKeys listed in `trusted_signers`; they MUST NOT reuse auth-callout User JWT signing material.

### WIT and WASI compatibility gate

| Scenario | Gateway behavior |
|----------|------------------|
| `target_wit = trogon:mcp-policy@0.1.0`, host supports `0.1.0` | Load |
| Bundle declares future minor within same major | Refuse unless host forward-compat table explicitly allows |
| Bundle declares future major | Refuse load; audit `policy_bundle_wit_incompatible` |
| `min_gateway_version` > running gateway | Refuse load with `gateway_too_old` |

Guest export `version()` returns manifest `version` for audit only; loader MUST NOT use it for compatibility ([mcp-policy-wit-sketch.md §Versioning](../identity/mcp-policy-wit-sketch.md#versioning-and-naming)).

### Size and failure defaults

| Cap | Default | Configurable |
|-----|---------|--------------|
| Single `components/*.wasm` | 32 MiB | per-tenant in gateway config |
| Total artifact | 64 MiB | per-tenant |
| `schemas/*.json` each | 16 KiB | no |
| `policies/*.cel` each | 256 KiB | no |

Cross-reference [failure-mode-matrix.md](../identity/failure-mode-matrix.md) rows 3–5: signature invalid, manifest invalid, and missing active bundle all default **closed**; prior bundle serves in-flight requests until drain completes.

## Status of supporting work

| Item | Status |
|------|--------|
| [wasm-bundle-format.md](../identity/wasm-bundle-format.md) reference spec | Draft (2026-05-28); aligned to this ADR with filename and NKey authoritative gate |
| [howto-write-bundle.md](../identity/howto-write-bundle.md) author guide | Draft; CEL-only path documented; WASM extension cross-ref |
| [mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md) host ABI | Block B paper artifact; `trogon:mcp-policy@0.1.0` pinned |
| [reference-host-abi.md](../identity/reference-host-abi.md) | Pending |
| Wasmtime integration + component pooling | **Pending** — Block F item 2 |
| Bundle loader in `trogon-mcp-gateway` | **Pending** — Block F item 5 |
| NKey verify in loader | **Pending** — Block F item 4 |
| `agctl mcp policies` sign/publish/verify | **Pending** — Block G |
| First-party `mcp-pack` bundle | **Pending** — Block F item 6 |
| KV bucket `mcp-policy-bundles` provisioning | **Pending** — operator automation |
| Audit `bundle_loaded` / `bundle_rejected` envelopes | **Pending** — Block F loader + audit schema update |
| Multi-source bundle composition (merging bundles from multiple KV sources at load time) | **Future work** — not on the critical path; revisit when a real composition use case appears |

---

*Design context: [wasm-bundle-format.md](../identity/wasm-bundle-format.md). Author workflow: [howto-write-bundle.md](../identity/howto-write-bundle.md). WIT contract: [mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md).*
