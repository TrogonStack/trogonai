# WASM policy bundle format (on-disk / on-wire)

**Status:** Block F paper artifact (2026-05-28). On-disk and on-wire layout for Phase 3 WASM policy bundles. No Wasmtime loader ships from this document alone.

**Document type:** Diátaxis **reference** (field tables, layout, validation rules, failure codes) with **explanation** prose where distribution and lifecycle trade-offs matter.

**Related:** [MCP policy WIT host ABI sketch](mcp-policy-wit-sketch.md), [Agent registry](registry.md), [Integration touch-points](integration-touchpoints.md), [Audit envelope schema reference](reference-audit-envelope.md), [Failure-mode matrix](failure-mode-matrix.md), [MCP gateway plan](../../MCP_GATEWAY_PLAN.md) Block F (Phase 3 WASM components + bundles).

**Implementation target:** `rsworkspace/crates/trogon-mcp-gateway` Phase 3 bundle loader; operator tooling in `agctl` (`Policies` subcommands — stubs today).

---

## 1. Goals

Phase 3 introduces **signed WASM policy components** that implement the guest exports in [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md). Before Wasmtime integration lands, this document pins how those components are **packaged**, **addressed**, **verified**, and **distributed**.

### 1.1 Design goals

| Goal | Requirement | Rationale |
|---|---|---|
| **Stable** | Layout and manifest schema version independently of guest code semver | Operators pin bundles by digest; gateway hot-swap must not reinterpret bytes ambiguously. |
| **Signed** | Mandatory cryptographic integrity before activation | Tenant WASM is untrusted code; supply-chain integrity is as important as sandboxing ([mcp-policy-wit-sketch.md § Trust boundary](mcp-policy-wit-sketch.md#trust-boundary)). |
| **Content-addressed** | Canonical bundle identifier is an immutable digest | Audit, rollback, and SIEM correlation reference one stable id across replicas and regions. |
| **Parseable without WASM execution** | Gateway and CI can validate manifest, signatures, and size limits before linking Wasmtime | Fail fast on bad bundles; offline `agctl` validation without executing guest code. |

### 1.2 Non-goals (this document)

- WIT host ABI details — see [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md).
- Tier 1 declarative YAML and Tier 2 CEL file layout inside a full `mcp-pack` — this spec covers the **Tier 3 WASM policy component artifact** and its envelope; a complete pack may embed additional layers **(proposed)** in future revisions.
- NATS subject constants for control-plane signals — cite [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) examples as **proposed** where not verified in repo ([reference-subject-grammar.md § proposed buckets](reference-subject-grammar.md#74-proposed-buckets)).

### 1.3 Terminology

| Term | Definition |
|---|---|
| **Bundle** | Signed OCI artifact (recommended) or KV-imported byte blob containing `bundle.toml`, `policy.wasm`, signatures, and optional metadata. |
| **Bundle digest** | SHA-256 content digest of the OCI manifest (see §4). Canonical identifier in audit and config pointers. |
| **Bundle id** | Human slug in `bundle.toml` (`acme/custom-policy`). Not globally unique without registry namespace; audit uses digest as authority. |
| **Policy component** | Single WASM Component Model artifact (`policy.wasm`) exporting `policy-guest` from WIT world `policy-bundle`. |
| **Target WIT** | Package pin in manifest, e.g. `trogon:mcp-policy@0.1.0`. Selects host linker, not guest `version()` export. |

---

## 2. Outer container

### 2.1 Recommendation: OCI artifact

**Production default:** publish bundles as **OCI artifacts** (ORAS / `cosign attach blob` / standard `application/vnd.oci.image.manifest.v1+json` with custom media types).

| Property | OCI artifact | Benefit |
|---|---|---|
| Distribution | Any OCI registry (GHCR, ECR, ACR, self-hosted) | Retention, geo-replication, and access control reuse existing artifact pipelines. |
| Signing | Sigstore cosign (keyless Fulcio or static key) | Transparency log (Rekor), certificate chains, and CI integration without inventing a custom envelope. |
| Content addressing | Manifest digest `sha256:…` | Native content-addressing; same id in registry, audit, and KV pointer. |
| Parsing without WASM | Fetch manifest + layers; validate `bundle.toml` before pulling `policy.wasm` | CI and gateway can reject oversize or incompatible bundles early. |

### 2.2 Alternatives considered

#### Raw `.wasm` in NATS KV

| Aspect | Assessment |
|---|---|
| **Pros** | Simple operator path for air-gapped clusters; aligns with existing JetStream KV patterns (`mcp-agent-registry`, `mcp-trust-bundles`). |
| **Cons** | No standard manifest layer; operators must store metadata in a separate KV key; signature story fragments (NKey per-key vs cosign); no registry retention or geo-replication. |
| **Verdict** | **Fallback import path** (§6.2), not production default. |

#### Tarball in object storage (S3 / GCS / JetStream Object Store)

| Aspect | Assessment |
|---|---|
| **Pros** | Familiar to release engineering; can hold multiple files; JetStream Object Store already mentioned in [MCP_GATEWAY_PLAN.md § Config Distribution](../../MCP_GATEWAY_PLAN.md) for binary WASM artifacts. |
| **Cons** | Signing and content-addressing are ad hoc unless wrapped in OCI anyway; bucket ACLs are separate from artifact signature verification; harder to standardize cosign/Rekor. |
| **Verdict** | Acceptable **staging** format for CI output, but **publish step should wrap tarball layers into OCI** before production promotion. Object Store may hold the same bytes as an OCI layer blob for NATS-centric fleets **(proposed)** — digest must still match OCI manifest. |

#### Comparison summary

| Criterion | OCI artifact | Raw WASM in KV | Tarball in S3 / Object Store |
|---|---|---|---|
| Registry-native distribution | Yes | No | Partial (custom) |
| cosign / Rekor | Yes | Manual | Manual |
| Content-addressed id | Manifest digest | KV revision + hash | ETag / custom |
| Parse without WASM | Yes (`bundle.toml` layer) | No (metadata split) | Yes if manifest inside |
| Air-gapped import | Via `oras copy` / offline tar | Yes | Yes |
| **Recommendation** | **Production default** | Offline fallback | CI staging only |

### 2.3 Media types **(proposed)**

| Layer file | OCI `mediaType` | Role |
|---|---|---|
| `bundle.toml` | `application/vnd.trogon.mcp-policy.bundle.v1+toml` | Manifest |
| `policy.wasm` | `application/vnd.trogon.mcp-policy.wasm.v1+wasm` | Guest component |
| `signatures/*` | `application/vnd.dev.cosign.simplesigning.v1+json` | cosign signature layers |
| `metadata/*` | `application/vnd.trogon.mcp-policy.metadata.v1+json` | Optional audit-only KV files |

Prefix `application/vnd.trogon.mcp-policy.*` is **proposed** until registered; loaders SHOULD accept generic `application/octet-stream` with path-based detection in dev only.

---

## 3. OCI artifact layout

An MCP WASM policy bundle OCI artifact contains exactly these logical paths in its filesystem layer (or as separate OCI layers — see §3.6):

```
/
├── bundle.toml          # required — manifest
├── policy.wasm          # required — WASM component
├── signatures/          # required — cosign material
│   ├── policy.sig       # required — signature over policy.wasm digest
│   └── bundle.sig       # required — signature over bundle.toml digest
│   └── *.sig            # optional — additional layer signatures
│   └── *.cert           # optional — certificate bundles (keyless)
└── metadata/            # optional — audit-surfaced, not enforced
    ├── owner.json
    └── …
```

### 3.1 `bundle.toml` — manifest

TOML encoding, UTF-8, no BOM. File MUST be present at artifact root. Gateway and CI parse this file before downloading or linking `policy.wasm`.

#### Field reference

| Field | Type | Required | Validation rule |
|---|---|---|---|
| `bundle_id` | string | yes | Pattern `^[a-z0-9][a-z0-9-]{0,62}/[a-z0-9][a-z0-9-]{0,62}$` — namespaced slug, lowercase, exactly one `/`. Same convention as [registry.md § Agent ID format](registry.md#agent-id-format--recommendation). |
| `bundle_version` | string (semver) | yes | Valid semver 2.0 (`MAJOR.MINOR.PATCH`, optional pre-release/build). Author-facing version; logged in audit; **not** the canonical bundle identifier (digest is). |
| `target_wit` | string | yes | Pattern `^[a-z0-9-]+:[a-z0-9-]+@[0-9]+\.[0-9]+\.[0-9]+$` — e.g. `trogon:mcp-policy@0.1.0`. Must match a host-supported WIT package pin ([mcp-policy-wit-sketch.md § Versioning](mcp-policy-wit-sketch.md#versioning-and-naming)). |
| `author` | string | yes | Non-empty, max 256 Unicode scalars. Owning team or publisher id (e.g. `platform-sre`, `acme-security`). |
| `created_at` | string (RFC3339) | yes | UTC timestamp with `Z` or numeric offset; must parse; SHOULD be ≤ signature timestamp. |
| `description` | string | yes | Non-empty, max 4096 Unicode scalars. Human summary for operator UI and audit `extra`. |
| `min_gateway_version` | string (semver) | yes | Minimum `trogon-mcp-gateway` semver that may activate this bundle. Gateway compares against its build version; refuse load if gateway too old. |

#### Optional manifest fields **(proposed)**

| Field | Type | Validation | Purpose |
|---|---|---|---|
| `capabilities.imports` | string[] | Each entry matches `host.[a-z0-9-]+` | Declared host imports; gateway refuses link if undeclared import used ([mcp-policy-wit-sketch.md § Capability manifest](mcp-policy-wit-sketch.md#capability-manifest)). |
| `capabilities.exports` | string[] | Each entry matches `policy-guest.[a-z0-9-]+` | Declared guest exports. |
| `wasm.mode` | enum | `authoritative` \| `advisory` | Whether WASM `evaluate` is sole PDP or advisory ([mcp-policy-wit-sketch.md § Pipeline position](mcp-policy-wit-sketch.md#pipeline-position)). Default `authoritative` **(proposed)**. |
| `max_policy_input_bytes` | u32 | ≤ host hard cap (1 MiB per field in WIT sketch) | Tenant-requested lower cap. |

#### Example `bundle.toml`

```toml
bundle_id = "acme/custom-policy"
bundle_version = "2.1.0"
target_wit = "trogon:mcp-policy@0.1.0"
author = "platform-sre"
created_at = "2026-05-28T12:00:00Z"
description = "Acme incident-response MCP policy component"
min_gateway_version = "0.3.0"

[capabilities]
imports = [
  "host.spicedb-check",
  "host.audit-emit",
  "host.time-now-ms",
]
exports = [
  "policy-guest.init",
  "policy-guest.version",
  "policy-guest.evaluate",
]

wasm.mode = "authoritative"
```

#### Manifest validation pipeline (reference)

1. Parse TOML; reject unknown **required** keys missing or wrong type.
2. Validate each field against regex / semver rules above.
3. Verify `target_wit` against gateway-supported WIT list (§7).
4. Verify cosign signatures over manifest bytes (§5) before trusting field values.
5. Optionally cross-check `bundle_id` against OCI annotation `org.opencontainers.image.title` **(proposed)** — mismatch is warning, not hard fail.

### 3.2 `policy.wasm` — WASM component

| Property | Rule |
|---|---|
| Format | WebAssembly **Component Model** (not legacy module-only), suitable for Wasmtime component linking. |
| World | MUST export WIT world `policy-bundle` for the pinned `target_wit` ([mcp-policy-wit-sketch.md § World definition](mcp-policy-wit-sketch.md#world-definition)). |
| Size | Default max **32 MiB** compressed layer size **(proposed)**; gateway refuses load above tenant-configured cap (§9.4). |
| Integrity | Layer digest recorded in OCI manifest; cosign signature in `signatures/policy.sig` (§5). |

Gateway MUST NOT execute `policy.wasm` during manifest-only validation. Component validation (WIT link preview) MAY run in CI or optional `--verify-component` loader flag **(proposed)**.

### 3.3 `signatures/` — cosign signatures

Directory holds Sigstore cosign signature material for mandatory layers.

| File | Signs | Required |
|---|---|---|
| `signatures/bundle.sig` | Digest of canonical `bundle.toml` bytes | yes |
| `signatures/policy.sig` | Digest of canonical `policy.wasm` bytes | yes |
| `signatures/*.cert` | Certificate bundle for keyless signatures | optional (required when using Fulcio) |
| `signatures/metadata/*.sig` | Optional metadata files | no |

Signature format: cosign **simple signing** JSON (`application/vnd.dev.cosign.simplesigning.v1+json`) or OCI cosign attachment manifest — loader MUST support at least detached `.sig` files co-located in the artifact tree for air-gapped import.

**Relationship to plan NKey manifest signing:** [MCP_GATEWAY_PLAN.md § Bundles](../../MCP_GATEWAY_PLAN.md) describes NKey signature over manifest digest. For OCI bundles, **cosign at the OCI layer is authoritative**; an optional `signatures/nkey.toml` **(proposed)** MAY duplicate NKey material for operators migrating from legacy KV bundles — gateway accepts either cosign OR NKey when both present, requiring **both** to verify **(proposed)**.

### 3.4 `metadata/` — arbitrary key/value

Optional directory of small JSON or plain-text files surfaced in audit and operator UI, **not enforced** by policy evaluation.

| Rule | Detail |
|---|---|
| Max files | 32 files **(proposed)** |
| Max file size | 16 KiB each **(proposed)** |
| Key namespace | Filename (without extension) becomes audit key, e.g. `metadata/owner.json` → `bundle_metadata.owner` |
| Enforcement | Gateway MUST NOT branch authorization logic on metadata contents. |
| Audit | On bundle activation, gateway MAY copy selected metadata into audit envelope `extra.bundle_metadata` **(proposed)** ([reference-audit-envelope.md](reference-audit-envelope.md)). |

Example:

```json
{
  "owner_team": "platform-sre",
  "slack_channel": "#mcp-policy",
  "change_ticket": "CHG-12345"
}
```

### 3.5 OCI annotations **(proposed)**

Recommended OCI manifest annotations for registry UI and promotion automation:

| Annotation key | Example value |
|---|---|
| `org.opencontainers.image.title` | `acme/custom-policy` |
| `org.opencontainers.image.version` | `2.1.0` |
| `org.opencontainers.image.created` | RFC3339 |
| `org.opencontainers.image.description` | Same as `description` field |
| `dev.trogon.mcp.target_wit` | `trogon:mcp-policy@0.1.0` |
| `dev.trogon.mcp.min_gateway_version` | `0.3.0` |

### 3.6 Layer packing strategies

Two valid OCI layouts:

| Strategy | Description | When to use |
|---|---|---|
| **Single layer tarball** | One `application/vnd.oci.image.layer.v1.tar+gzip` containing full tree above | Simplest cosign `sign blob`; air-gapped `oras copy`. |
| **Multi-layer** | Separate layers for `bundle.toml`, `policy.wasm`, `signatures/`, `metadata/` | Larger WASM changes do not re-upload manifest layer; better CDN caching. |

In both cases, **content-addressed bundle id** is the digest of the **OCI image manifest** listing those layers (§4), not an individual layer digest.

---

## 4. Content-addressing

### 4.1 Canonical bundle identifier

The **canonical bundle identifier** is the OCI **image manifest digest**:

```text
sha256:<hex>   # e.g. sha256:6c1f6b2e…
```

Rules:

1. Computed over the canonical JSON of the OCI manifest (registry-spec deterministic serialization).
2. Used in all gateway audit envelopes that reference an activated policy bundle.
3. Used in KV active-pointer values (§6.2) and config promotion records **(proposed)**.
4. Human tags (`:2.1.0`, `:latest`) are convenience aliases only; audit and rollback pin **digest**.

### 4.2 Audit envelope fields **(proposed)**

Extend gateway audit ([reference-audit-envelope.md](reference-audit-envelope.md)) with:

| Field | Type | Source |
|---|---|---|
| `policy_bundle_digest` | string | OCI manifest digest |
| `policy_bundle_id` | string | `bundle.toml` `bundle_id` |
| `policy_bundle_version` | string | `bundle.toml` `bundle_version` |
| `policy_target_wit` | string | `bundle.toml` `target_wit` |
| `policy_signer_identity` | string | cosign certificate subject or key id |

Every evaluation on the WASM tier SHOULD repeat `policy_bundle_digest` in JSON-RPC `error.data` for `-32100`, `-32101`, `-32108` paths ([mcp-policy-wit-sketch.md § JSON-RPC error.data](mcp-policy-wit-sketch.md#error-mapping)).

### 4.3 Immutability

Published digests are immutable. A new `bundle_version` with different bytes MUST produce a new digest. Operators never mutate artifact content in-place; promotion is pointer update only (§6, §8).

### 4.4 Relationship to `bundle_id` / semver

| Identifier | Mutable? | Role |
|---|---|---|
| OCI digest | No | Authoritative correlation, rollback pin |
| `bundle_id` | Stable slug across versions | Human operator reference |
| `bundle_version` | Changes per release | Semver policy (§7); audit only |
| OCI tag | Mutable pointer | Promotion channel; not trusted without digest pin |

---

## 5. Signing

Signing is **mandatory**. Unsigned bundles MUST NOT activate in production ([failure-mode-matrix.md](failure-mode-matrix.md) row 4, invariant 8).

### 5.1 Supported verification modes

| Mode | Mechanism | Typical use |
|---|---|---|
| **cosign keyless** | Fulcio certificate + Rekor transparency log entry | CI/CD from GitHub Actions / GitLab; short-lived certs bound to OIDC identity. |
| **cosign static key** | `cosign sign --key` / KMS-backed key | Air-gapped or long-lived org signing key; no Fulcio dependency. |

Both modes MAY coexist in an organization; gateway trust policy selects allowed identities (§5.3).

### 5.2 Verification surface

At load time the gateway (or CI validator) MUST:

1. Verify cosign signature over **`bundle.toml`** digest (`signatures/bundle.sig`).
2. Verify cosign signature over **`policy.wasm`** digest (`signatures/policy.sig`).
3. Validate signer identity against configured trust roots.
4. For keyless: verify certificate chain to Fulcio root **and** check Rekor log inclusion **(proposed default: required in production)**.
5. Reject expired certificates (keyless) or revoked keys (static CRL/OCSP if configured **(proposed)**).

#### Trust roots **(proposed configuration)**

| Config source | Content |
|---|---|
| NATS KV `mcp-gateway-config/trusted_signers` | Allowed cosign public keys (PEM) and/or Fulcio certificate identities (email, OIDC issuer/subject patterns) — aligns with [mcp-policy-wit-sketch.md § Signer trust](mcp-policy-wit-sketch.md#signer-trust). |
| Environment bootstrap | `MCP_GATEWAY_POLICY_TRUST_ROOTS_PATH` — file or directory for offline/air-gapped |

Unknown signer → load rejected; previous bundle remains active ([failure-mode-matrix.md](failure-mode-matrix.md) row 4).

#### Transparency log

| Setting | Production default **(proposed)** |
|---|---|
| Rekor verification for keyless | **Required** |
| Rekor for static keys | Optional (org policy) |
| Offline/air-gapped | Static keys only; Rekor skipped with explicit operator flag `MCP_GATEWAY_POLICY_SKIP_REKOR=1` (dev/break-glass only) |

### 5.3 Verification algorithm (reference)

```text
LOAD(artifact_ref):
  manifest, layers ← pull OCI artifact
  REQUIRE layers contain bundle.toml AND policy.wasm
  REQUIRE signatures/bundle.sig AND signatures/policy.sig exist

  manifest_bytes ← read bundle.toml
  wasm_bytes ← read policy.wasm

  VERIFY cosign(manifest_bytes, bundle.sig, trust_policy)
  VERIFY cosign(wasm_bytes, policy.sig, trust_policy)

  PARSE bundle.toml → manifest_record
  VALIDATE manifest_record fields (§3.1)
  CHECK target_wit compatibility (§7)

  IF all ok → eligible for activation
  ELSE → reject; emit mcp.control.bundle.load_failed (proposed)
```

### 5.4 CI vs gateway verification

| Stage | Responsibility |
|---|---|
| **CI publish** | Sign layers; reject publish if cosign or manifest lint fails. |
| **Registry admission** | Optional policy controller (OPA, rego) on OCI annotations **(proposed)**. |
| **Gateway load** | Re-verify signatures (do not trust registry alone). |
| **agctl offline** | `agctl mcp policies verify --bundle path.or.ref` **(proposed)** — same checks without Wasmtime. |

---

## 6. Distribution

Two pull paths: **OCI registry (production default)** and **NATS KV import (offline / air-gapped fallback)**.

### 6.1 Path A — OCI registry pull (production default)

Gateway pulls from a configured OCI registry by **digest** (recommended) or **tag** (discouraged without subsequent digest pin).

| Config **(proposed)** | Meaning |
|---|---|
| `MCP_GATEWAY_POLICY_OCI_REGISTRY` | Registry host, e.g. `ghcr.io/acme/mcp-policy` |
| `MCP_GATEWAY_POLICY_OCI_REF` | `sha256:…` or tag `@digest` pin |
| `MCP_GATEWAY_POLICY_OCI_AUTH` | Registry credentials via env / credential helper |

Flow:

1. Operator publishes bundle to OCI (CI).
2. Operator updates active digest pointer in `mcp-gateway-config` **(proposed key:** `policy/wasm/active_digest`) or promotion API.
3. Gateway watcher fires; loader pulls OCI artifact by digest, verifies (§5), warms pool (§8), flips active pointer.
4. Audit records `policy_bundle_digest` on activation.

**Why default:** Retention, geo-replication, cosign/Rekor integration, and separation of binary blobs from JetStream KV IOPS ([mcp-gateway-operator-overview.md § JetStream KV throughput](mcp-gateway-operator-overview.md)).

### 6.2 Path B — NATS KV import (offline / air-gapped fallback)

For clusters without outbound registry access, operators import a pre-verified OCI artifact tarball (or raw layout tree) into JetStream KV.

#### KV bucket name **(proposed)**

Grep of this repository shows **no** existing constant `mcp-policy-bundles`. Related buckets today: `mcp-gateway-config`, `mcp-agent-registry`, `mcp-trust-bundles` ([reference-subject-grammar.md §7](reference-subject-grammar.md)).

**Proposed bucket:** `mcp-policy-bundles`

| Property | Value **(proposed)** |
|---|---|
| Bucket | `mcp-policy-bundles` |
| Key | `{bundle_digest}` — full `sha256:…` string |
| Value | OCI artifact tarball bytes (single layer) OR canonical serialized artifact |
| Max value | 64 MiB (fits 32 MiB WASM + manifest + signatures) |
| Writers | Operator tooling, import controller — not gateway |
| Readers | Gateway loader only |

Active pointer remains in `mcp-gateway-config` **(proposed):**

```json
{
  "source": "kv",
  "digest": "sha256:6c1f6b2e…",
  "bundle_id": "acme/custom-policy",
  "bundle_version": "2.1.0"
}
```

Import flow:

1. Operator runs offline `cosign verify` + `agctl mcp policies import --file bundle.tar --digest sha256:…` **(proposed)**.
2. Tool writes KV key `mcp-policy-bundles/{digest}`.
3. Operator updates `mcp-gateway-config` active pointer to digest.
4. Gateway loads from KV, **re-verifies signatures** (same as OCI path).

**JetStream Object Store:** [MCP_GATEWAY_PLAN.md § Config Distribution](../../MCP_GATEWAY_PLAN.md) mentions Object Store for WASM binaries. Object Store MAY hold the same bytes keyed by digest **(proposed)** with KV holding only the active pointer — equivalent to Path B if loader treats Object Store as blob backend.

### 6.3 Path selection

| Environment | Recommended path |
|---|---|
| Production multi-region | OCI registry + digest pin |
| Single-cluster NATS-centric | OCI pull if egress allowed; else KV import |
| Air-gapped | KV import after offline verify |
| Dev | Local directory or `oras push` to dev registry |

Gateway config **(proposed):** `MCP_GATEWAY_POLICY_SOURCE=oci|kv|file` with `oci` default.

### 6.4 Multi-source layering

Aligns with [MCP_GATEWAY_PLAN.md § Bundles](../../MCP_GATEWAY_PLAN.md): org-wide base + tenant override.

| Layer | Source **(proposed)** | Precedence |
|---|---|---|
| Org base WASM policy | OCI `ghcr.io/org/mcp-policy-base@sha256:…` | lowest |
| Tenant override | OCI or KV digest per tenant | highest |

Merge semantics for WASM tier: **most specific wins** — tenant digest replaces org digest for that tenant's gateway instances; not a merge of WASM bytecode ([hierarchical-policy-merge.md](hierarchical-policy-merge.md) CEL layering is separate).

---

## 7. Versioning

Two independent version axes: **`bundle_version`** (author semver in manifest) and **`target_wit`** (host ABI pin). Cross-reference [mcp-policy-wit-sketch.md § Semver guarantees](mcp-policy-wit-sketch.md#semver-guarantees).

### 7.1 `bundle_version` (author semver)

| Bump | Meaning | Gateway action |
|---|---|---|
| **Patch** | Bugfix in guest logic; same `target_wit` and capabilities | Hot-swap allowed; no config change |
| **Minor** | New policy behavior; same `target_wit` | Hot-swap allowed; audit notes version change |
| **Major** | Breaking policy semantics (operator-defined) | Hot-swap allowed; runbook may require canary **(proposed)** |

`bundle_version` does **not** select the Wasmtime linker — only `target_wit` does.

### 7.2 `target_wit` compatibility

Parse `target_wit` as `{package}@{major}.{minor}.{patch}`.

| Scenario | Gateway behavior |
|---|---|
| `target_wit` = supported pin exactly (e.g. host supports `trogon:mcp-policy@0.1.0`) | Load |
| `target_wit` **future minor** within same major (bundle declares `0.2.0`, host supports `0.1.0`) | **Forward-compat OK** if host implements semver rules: minors only add optional fields/imports; guest compiled against 0.2.0 runs on 0.1.0 host only if guest uses subset — **loader SHOULD warn** and accept when host minor ≥ required **(proposed)** OR when host declares forward-compat table |
| `target_wit` **future major** (bundle `1.0.0`, host supports `0.x`) | **Refuse load** — `-32108` / unhealthy until operator downgrades bundle or upgrades gateway |
| `target_wit` **past major** (bundle `0.1.0`, host `1.0.0`) | Load if host maintains backward compat shim **(proposed)**; else refuse |

**Normative rule for Phase 3 MVP:** gateway maintains explicit allowlist of supported `target_wit` pins. Forward minor: accept when `bundle.minor ≤ host.minor` AND `bundle.major == host.major`. Future major: **refuse**.

### 7.3 `min_gateway_version`

Independent gate: if `min_gateway_version` > running gateway semver, refuse load with reason `gateway_too_old` **(proposed)** even when `target_wit` matches.

### 7.4 Guest `version()` export

Returns `bundle_version` for audit only ([mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md)). Loader MUST NOT use it for compatibility decisions.

---

## 8. Loading

### 8.1 Loader phases

| Phase | Work | WASM execution? |
|---|---|---|
| **Fetch** | OCI pull or KV get by digest | No |
| **Verify** | cosign + manifest validation | No |
| **Compile** | Wasmtime component compile / cache | No guest code |
| **Pool** | Instantiate N components; call `init()` | Yes (guest init) |
| **Activate** | Atomic flip of active digest pointer | No |
| **Evaluate** | Borrow pool instance; `evaluate(input)` | Yes (request path) |

### 8.2 Cold-load latency budget **(proposed)**

Targets for single bundle on amd64/linux, warm disk cache, Wasmtime 21+:

| Step | P50 budget | P99 budget |
|---|---|---|
| OCI pull (1 MiB WASM) | 200 ms | 800 ms |
| cosign verify (2 signatures) | 20 ms | 50 ms |
| Manifest parse | 5 ms | 10 ms |
| Wasmtime compile + link | 150 ms | 400 ms |
| Pool init (`N=4`, `init()` each) | 80 ms | 200 ms |
| **Total cold load** | **455 ms** | **1.46 s** |

KV path skips network pull; add 0–50 ms for KV get. Exceeding P99 during promotion SHOULD mark instance `/ready=false` until warm completes **(proposed)**.

### 8.3 Warm cache

| Cache layer | Scope | Invalidation |
|---|---|---|
| OCI layer cache | Per gateway node disk | LRU; digest change |
| Wasmtime compiled component | Per `{digest, target_wit}` | Digest change |
| Instance pool | Per active digest | Hot-swap |

Previous digest remains in memory until in-flight evaluations complete ([MCP_GATEWAY_PLAN.md § Hot-swap](../../MCP_GATEWAY_PLAN.md)).

### 8.4 Pre-fetch and promotion **(proposed)**

When operator promotes bundle digest `D_new`:

1. Publish `mcp.control.bundle.prefetch` **(proposed)** with `{ "digest": "D_new" }` OR update KV pointer with `prefetch_only: true` flag.
2. All gateway replicas fetch, verify, compile, and pool **without** flipping active pointer.
3. Operator confirms canary metrics; sets `active_digest = D_new`.
4. Replicas flip atomically; drain old pool after quiescence.

This avoids cold-load latency on first production request after promotion.

### 8.5 Hot-swap semantics

| Event | Behavior |
|---|---|
| New digest verifies | Old digest serves in-flight; new digest takes new requests |
| New digest fails verify | Old digest unchanged; `/ready` false if no old digest |
| Rollback | Repoint active digest to `D_old`; optional KV/OCI tag move |

Signal: `mcp.control.bundle.reload` ([MCP_GATEWAY_PLAN.md § Control subjects](../../MCP_GATEWAY_PLAN.md)) — belt-and-braces; KV watcher is primary.

---

## 9. Failure modes

Cross-reference [failure-mode-matrix.md](failure-mode-matrix.md) rows 3–5.

### 9.1 Signature invalid

| Trigger | cosign verify fail; unknown signer; Rekor check fail |
|---|---|
| Default behaviour | **CLOSED** for new bundle; prior bundle stays active |
| Request path | If no prior bundle: `-32108` `no_policy` **(proposed)** |
| Audit | `mcp.control.bundle.load_failed` **(proposed)** with `decision_reason=policy_bundle_signature_invalid` |
| Operator action | Fix signing pipeline; verify trust roots in `mcp-gateway-config/trusted_signers` |

### 9.2 Manifest missing or invalid

| Trigger | `bundle.toml` absent; TOML parse error; field validation fail |
|---|---|
| Default behaviour | **CLOSED** — reject artifact |
| Request path | Same as §9.1 |
| Audit | `policy_bundle_manifest_invalid` **(proposed)** |
| Operator action | Republish OCI artifact; run `agctl mcp policies verify` **(proposed)** in CI |

### 9.3 `target_wit` incompatible

| Trigger | Future major WIT; unsupported package; missing host import |
|---|---|
| Default behaviour | **CLOSED** — refuse link |
| Request path | Prior bundle or `-32108` |
| Audit | `policy_bundle_wit_incompatible` **(proposed)** with `target_wit` in envelope |
| Operator action | Upgrade gateway or publish bundle compiled for supported WIT pin |

### 9.4 Bundle too large

| Trigger | `policy.wasm` or total artifact exceeds cap |
|---|---|
| Default cap **(proposed)** | WASM 32 MiB; total artifact 64 MiB; configurable per tenant |
| Default behaviour | **CLOSED** at load |
| Request path | `-32108` or prior bundle |
| Audit | `policy_bundle_oversize` **(proposed)** |
| Operator action | Shrink guest; split redaction into separate component world **(future)** |

### 9.5 Runtime failures (cross-ref)

| Class | Matrix row | JSON-RPC |
|---|---|---|
| WASM trap on request | Row 3 | `-32101` `policy_fault` **(proposed)** |
| Invalid signature at hot-swap | Row 4 | N/A request / unhealthy |
| Missing active bundle | Row 5 | `-32108` `no_policy` **(proposed)** |

---

## 10. Cross-references

| Topic | Document / anchor |
|---|---|
| WIT host ABI, `target_wit`, guest exports | [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md) |
| Agent id namespace (parallel to `bundle_id`) | [registry.md](registry.md) |
| Gateway integrations, KV buckets, STS | [integration-touchpoints.md](integration-touchpoints.md) |
| Audit fields for bundle digest | [reference-audit-envelope.md](reference-audit-envelope.md) |
| Failure defaults (rows 3–5) | [failure-mode-matrix.md](failure-mode-matrix.md) |
| Block F checklist (bundle format item) | [MCP_GATEWAY_PLAN.md Block F](../../MCP_GATEWAY_PLAN.md) |
| Operator KV overview | [mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md) |
| Hierarchical policy (CEL layers vs WASM) | [hierarchical-policy-merge.md](hierarchical-policy-merge.md) |
| Bootstrap / day-zero bundle pointer | [bootstrap-day-zero.md](bootstrap-day-zero.md) |
| A2A signed bundle precedent (Ed25519) | `rsworkspace/crates/a2a-redaction/src/signed_bundle/` — different format; inspiration only |

---

## Appendix A — OCI pull example (non-normative)

```bash
# Publish (CI)
cosign sign-blob --bundle signatures/bundle.sig bundle.toml
cosign sign-blob --bundle signatures/policy.sig policy.wasm
oras push ghcr.io/acme/mcp-policy:2.1.0 \
  bundle.toml:application/vnd.trogon.mcp-policy.bundle.v1+toml \
  policy.wasm:application/vnd.trogon.mcp-policy.wasm.v1+wasm

# Promote by digest
DIGEST="$(oras manifest fetch ghcr.io/acme/mcp-policy:2.1.0 | jq -r .digest)"
# Update mcp-gateway-config active pointer to $DIGEST (proposed tooling)
```

---

## Appendix B — KV import example **(proposed, non-normative)**

```bash
# Offline verify then import
cosign verify-blob --certificate-identity-regexp '.*@acme.com' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --bundle signatures/bundle.sig bundle.toml

nats kv put mcp-policy-bundles "sha256:6c1f6b2e…" @bundle-artifact.tar.gz
nats kv put mcp-gateway-config policy/wasm/active_digest @active-pointer.json
```

---

## Appendix C — Active pointer JSON schema **(proposed)**

```json
{
  "source": "oci",
  "digest": "sha256:6c1f6b2e4d8a1c0f9e3b7a2d5c8f1e4b7a0d3c6f9e2b5a8d1c4f7e0b3a6d9c2",
  "oci_ref": "ghcr.io/acme/mcp-policy@sha256:6c1f6b2e…",
  "bundle_id": "acme/custom-policy",
  "bundle_version": "2.1.0",
  "target_wit": "trogon:mcp-policy@0.1.0",
  "promoted_at": "2026-05-28T14:30:00Z",
  "promoted_by": "human:alice@acme"
}
```

---

## Appendix D — Block F checklist mapping

| [MCP_GATEWAY_PLAN.md Block F](../../MCP_GATEWAY_PLAN.md) item | Satisfied by this doc |
|---|---|
| Bundle format (manifest + WASM components) | §3 layout, `bundle.toml`, `policy.wasm` |
| NKey signature verification | §5 (cosign primary; NKey optional migration) |
| Bundle loader from NATS KV | §6.2 Path B |
| Hot-swap and rollback | §8.5 |
| WIT finalized | Cross-ref [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md); `target_wit` field |
| First-party `mcp-pack` | Uses same format; additional tiers **(proposed)** as separate layers later |

---

## Appendix E — Security notes and open questions

**Security:** Never skip verify on KV import. SIEM rules SHOULD key on `policy_bundle_digest`, not `bundle_version`. Metadata is display-only — no authz branching. Tag `:latest` alone is insufficient; always record digest in the active pointer.

**Open:** (1) Full `mcp-pack` single-artifact vs split tiers. (2) NKey-only air-gapped policy vs dual cosign+NKey. (3) Component Model only at load — legacy modules rejected **(proposed)**. (4) Shared OCI layout with A2A redaction bundles when `trogon-policy-core` extracts **(open)**.

---

## Revision history

| Date | Change |
|---|---|
| 2026-05-28 | Initial Block F paper: OCI layout, `bundle.toml`, cosign, digest identity, OCI vs KV distribution |

---

*End of reference.*
