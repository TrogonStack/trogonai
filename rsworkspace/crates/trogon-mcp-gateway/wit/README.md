# `trogon:mcp-policy@0.1.0` WIT package

Canonical WebAssembly Component Model interface for MCP gateway Tier-3 policy bundles.

| Field | Value |
|---|---|
| Package | `trogon:mcp-policy@0.1.0` |
| World | `policy-bundle` |
| WASI pin | `wasi:cli/imports@0.3.0-rc-2025-09-16` (WASI 0.3 draft) |
| Source file | [`trogon-mcp-policy.wit`](trogon-mcp-policy.wit) |
| Rust mirror | [`../src/wasm/bindings.rs`](../src/wasm/bindings.rs) |
| Block B sketch (superseded) | [`docs/identity/mcp-policy-wit-sketch.md`](../../../docs/identity/mcp-policy-wit-sketch.md) |
| Host ABI reference | [`docs/identity/reference-host-abi.md`](../../../docs/identity/reference-host-abi.md) |

## Validation

```bash
wasm-tools component wit rsworkspace/crates/trogon-mcp-gateway/wit/
```

Validated with `wasm-tools` on 2026-05-29 (`exit 0`). `wit-deps` is not required; deps are vendored under [`deps/`](deps/).

## Vendored WASI dependencies

Snapshot tag **`v0.3.0-rc-2025-09-16`** from the WebAssembly WASI repos (aligned release candidate for Preview 3):

| Package | Source repository | Vendored path |
|---|---|---|
| `wasi:cli@0.3.0-rc-2025-09-16` | [WebAssembly/wasi-cli](https://github.com/WebAssembly/wasi-cli) | `deps/cli/` |
| `wasi:clocks@0.3.0-rc-2025-09-16` | [WebAssembly/wasi-clocks](https://github.com/WebAssembly/wasi-clocks) | `deps/clocks/` |
| `wasi:filesystem@0.3.0-rc-2025-09-16` | [WebAssembly/wasi-filesystem](https://github.com/WebAssembly/wasi-filesystem) | `deps/filesystem/` |
| `wasi:random@0.3.0-rc-2025-09-16` | [WebAssembly/wasi-random](https://github.com/WebAssembly/wasi-random) | `deps/random/` |
| `wasi:sockets@0.3.0-rc-2025-09-16` | [WebAssembly/wasi-sockets](https://github.com/WebAssembly/wasi-sockets) | `deps/sockets/` |

Manifest pin in bundle `target_wit` remains **`trogon:mcp-policy@0.1.0`**. The WASI RC suffix is an implementation detail of the vendored snapshot; gateway linkers accept this RC as the Phase 3 **WASI 0.3** pin per Block F item 1.

To refresh deps, re-download each repo at `v0.3.0-rc-2025-09-16` and copy `wit-0.3.0-draft/` into the matching `deps/<name>/` directory, preserving nested `deps/` symlinks under `deps/cli/`.

## Version policy

Semver applies to the **WIT package** (`trogon:mcp-policy`), not the guest export `version()` string.

| Change class | Version bump | Examples |
|---|---|---|
| **Additive** | Minor (`0.1.x`, `0.2.0` within major `0`) | New host import (`kv-fetch`), new optional `request-ctx` field at record end, new enum variant, new guest export |
| **Breaking** | Major (`1.0.0`, …) | Remove import, rename export, change function signature, reorder existing record fields |
| **Guest `version()`** | None (audit only) | Bundle manifest `version`; never selects linker |

Rules:

1. Minor releases **must not** remove, rename, or change signatures of symbols present in prior minors within the same major.
2. Additive record fields append at the **end** with documented defaults at the ABI layer.
3. Gateways may load multiple **major** WIT versions side-by-side during migration via manifest `target_wit` / `wit_major` **(proposed)**.
4. Bundle manifest `target_wit` selects the host linker; guest `policy-guest.version()` is for audit only.

## Host ABI mapping

WIT imports use kebab-case; CEL builtins use dotted namespaces. Semantics match [`reference-host-abi.md`](../../../docs/identity/reference-host-abi.md).

| CEL builtin | WIT import | Notes |
|---|---|---|
| `spicedb.check(subject, permission, resource)` | `host.spicedb-check` | WIT arg `object-id` = CEL `resource` |
| `cache.get` / `cache.set` | `host.cache-get` / `host.cache-set` | CEL JSON ↔ WIT `list<u8>` at boundary |
| `audit.emit(map)` | `host.audit-emit` | CEL map → `fields-json`; WIT adds `category` |
| `time.now()` | `host.time-now` | Milliseconds since Unix epoch |
| `rate.acquire(scope, key, budget, window)` | `host.rate-acquire` | `window` → `window-secs` |
| `jsonpath.extract(json, path)` | `host.jsonpath-read` | Exactly one match |
| `jsonpath.has(json, path)` | `host.jsonpath-has` | Boolean presence |
| *(tracing)* | `host.span-attribute-set`, `host.span-event` | ADR 0032; optional at link time |
| *(telemetry)* | `host.log` | Non-authorization side effect |

**Excluded from v0.1.0** (future minors): `spicedb.bulk_check`, `kv.fetch`, `nats.request`, `time.hour_utc`, `time.weekday`.

### Guest exports

| Export | Rust trait method | Purpose |
|---|---|---|
| `init` | `PolicyGuest::init` | Once per pooled instance |
| `version` | `PolicyGuest::version` | Author semver (audit) |
| `evaluate` | `PolicyGuest::evaluate` | Primary policy decision |
| `materialize-bindings` | `PolicyGuest::materialize_bindings` | Optional SpiceDB tuple derivation |

### Types

| WIT type | Rust type | Location |
|---|---|---|
| `log-level` | `LogLevel` | `bindings.rs` |
| `host-failure` | `HostFailure` | `bindings.rs` |
| `span-context` | `SpanContext` | `bindings.rs` |
| `tool-descriptor` | `ToolDescriptor` | `bindings.rs` |
| `request-ctx` | `RequestCtx` | `bindings.rs` |
| `policy-decision` | `PolicyDecision` | `bindings.rs` |
| `spicedb-binding` | `SpicedbBinding` | `bindings.rs` |

Block B sketch name `policy-input` is superseded by **`request-ctx`** with an explicit `span-context` field (ADR 0032). Block B sketch name `host-error` is superseded by **`host-failure`**.

## Worlds and interfaces

| Name | Kind | Role |
|---|---|---|
| `policy-types` | interface | Shared records and enums |
| `host` | interface | Gateway-provided imports |
| `policy-guest` | interface | Component exports |
| `policy-bundle` | world | Bundle link target (`include wasi:cli/imports`, `import host`, `export policy-guest`) |

## Capability gating

Each `host.*` import must appear in bundle manifest `capabilities.imports`. Undeclared imports fail at link time or return `host-failure.code = undeclared_host_capability` at runtime. See reference-host-abi §12.
