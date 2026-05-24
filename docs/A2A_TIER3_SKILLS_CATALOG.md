# Tier-3 skills catalog

Operator-facing reference for the Wasmtime-hosted Tier-3 redaction skill matrix. Manifest schema, registry loading, and bundled reference stubs live in `a2a-redaction` and `a2a-pack`; WASM compilation and gateway dispatch wiring are separate workstreams.

## Layout

| Path | Role |
|------|------|
| `rsworkspace/crates/a2a-redaction/src/skill_manifest.rs` | `SkillManifest`, `SkillManifestRegistry`, `SkillSelectionPlan` |
| `rsworkspace/crates/a2a-pack/skills/` | First-party reference manifests + README stubs (no compiled WASM) |

Gateway preload continues to use `A2A_GATEWAY_POLICY_BUNDLE_DIR` for `.wasm` binaries and `A2A_GATEWAY_POLICY_SKILLS` for explicit preload slugs. Skill manifests are loaded from the same bundle directory (or a subdirectory — see [Runtime env](./A2A_RUNTIME_ENV.md)).

## Skill id naming

Use dot-separated slugs:

```text
<category>.<purpose>.<vN>
```

Examples:

- `pii.email_mask.v1`
- `credentials.bearer_redact.v1`
- `internal_route.x_internal_strip.v1`

The manifest filename must match: `<skill_id>.skill.toml`.

## Manifest schema (TOML)

One file per skill at `<bundle_dir>/<skill_id>.skill.toml`:

```toml
skill_id = "pii.email_mask.v1"
wasm_path = "skills/pii_email_mask.wasm"
applies_to_method = { kind = "OneOf", methods = ["message/send", "message/stream"] }
applies_to_paths = ["$.message.parts[*].text"]
category = "Pii"
version = "1.0.0"
```

| Field | Type | Notes |
|-------|------|-------|
| `skill_id` | string | Must match filename stem before `.skill` |
| `wasm_path` | string | Relative to bundle root |
| `applies_to_method` | `Any` or `{ kind = "OneOf", methods = [...] }` | Methods use A2A JSON-RPC names (`message/send`, …) |
| `applies_to_paths` | string array | JSONPath-style selectors; at least one required |
| `category` | enum string or `{ Custom = "label" }` | Drives selection order (see below) |
| `version` | semver-shaped string | e.g. `1.0.0`, `1.2.3-rc.1` |

Malformed manifests (missing fields, invalid version, filename/skill_id mismatch, duplicate `skill_id`) fail closed at load time via `SkillManifestError`.

## Method matchers

- **`Any`** — skill applies to every A2A method.
- **`OneOf`** — skill applies only when the ingress method is listed. Method strings match the gateway/A2A JSON-RPC surface (`message/send`, `tasks/get`, `agent/card`, …).

## JSON path expressions

`applies_to_paths` entries are JSONPath-style strings (e.g. `$.message.parts[*].text`). The Tier-3 call site (future) extracts paths present in the payload; `SkillSelectionPlan` includes a skill when any manifest path intersects the payload path set.

## Selection order

`SkillSelectionPlan::plan(registry, method, payload_paths)` returns manifests in deterministic order:

1. Category priority: `InternalRoute` → `Credentials` → `Pii` → `RateLimit` → `Custom`
2. Tie-break: `skill_id` lexicographic ascending

## Guest ABI

Each WASM skill exports:

```text
redact_part(in_ptr, in_len) -> (out_ptr, out_len)
```

The host passes UTF-8 JSON fragments for values at matched paths; the guest returns the redacted fragment with the same JSON shape.

## Reference skills (intent only)

Bundled under `a2a-pack/skills/`. WASM authoring is operator work; manifests and READMEs define the contract.

### `pii.email_mask.v1`

Mask email-shaped substrings in part text (`alice@example.com` → `[REDACTED_EMAIL]`). Targets `message/send` and `message/stream` part text/content paths.

### `credentials.bearer_redact.v1`

Strip `Authorization: Bearer …` substrings from part text. Targets the same message methods and text paths as PII masking.

### `internal_route.x_internal_strip.v1`

Remove `x-internal-*` keys from metadata/extension objects. Applies to `message/send`, `message/stream`, and `agent/card`.

## Loading in Rust

```rust
use a2a_redaction::SkillManifestRegistry;

let registry = SkillManifestRegistry::load_from_dir(bundle_dir)?;
let plan = a2a_redaction::SkillSelectionPlan::plan(
    &registry,
    &a2a_redaction::A2aMethod::MessageSend,
    &payload_paths,
);
```

## Related docs

- [Runtime environment](./A2A_RUNTIME_ENV.md) — `A2A_GATEWAY_POLICY_BUNDLE_DIR`, `A2A_GATEWAY_POLICY_SKILLS`
- [Open work tracker](../A2A_TODO.md) — Phase 2 skill matrix status
