# mcp-pack

Builds the canonical first-party MCP policy bundle consumed by `trogon-mcp-gateway::bundle::load_bundle`. The pack ships Tier-2 CEL defaults and a Tier-3 `schema-learner` WASM stub per [ADR 0028](../../docs/adr/0028-mcp-pack-bundle.md) and [ADR 0010](../../docs/adr/0010-bundle-format.md).

## Bundle contents (v0.1.0)

| Member | Program / component id | Role |
|--------|----------------------|------|
| `policies/default_catalog_filter.cel` | `mcp-pack/list-tools` | `tools_list_filter` — hides tools matching deny patterns on `mcp.tool.name` |
| `policies/default_resource_tuple.cel` | `mcp-pack/resource-tuple` | `ingress_gate` — SpiceDB checks from `mcp.method` + JSONPath into `mcp.params` |
| `policies/default_audit.cel` | `mcp-pack/audit-default` | `audit_enrich` — default Pin-7-style `audit.emit` extras |
| `components/schema-learner.wasm` | `schema-learner` | Advisory WASM stub (no host logic; placeholder for Wasmtime wiring) |

Manifest `name` is `global/mcp-pack` (ADR `_global/mcp-pack` without underscore until gateway scope rules allow `_` in tenant segments).

## Library usage

```rust
use mcp_pack::{McpPack, McpPackSpec};
use trogon_mcp_gateway::bundle::{load_bundle, TrustedKeys};

let spec = McpPackSpec::with_ephemeral_signer();
let trusted = TrustedKeys::from_allowlist([spec.signer.public_key()]);
let bytes = McpPack::new(spec).build()?;
let loaded = load_bundle(&bytes, &trusted)?;
```

Use a stable Operator NKey in production (`KeyPair::from_seed(...)`) and pin the public key in gateway `trusted_signers`.

## CLI

```bash
cargo run -p mcp-pack -- build --out ./pack.bundle
```

Writes an **uncompressed tar** archive (gateway loader format). Sign with the same NKey whose public half is listed in `manifest.toml` `[signing].nkey_pub` and in gateway trust config.

## Rebuild `schema-learner` stub

The committed stub is a minimal WASM 1.0 module (`\0asm\x01\0\0\0`) for loader round-trips only. It does not implement `trogon:mcp-policy@0.1.0` exports.

To replace it with a real component (when `cargo component` is available in your environment):

1. Create a guest crate targeting `trogon:mcp-policy@0.1.0` / world `policy-bundle` (see `trogon-mcp-gateway/wit/trogon-mcp-policy.wit`).
2. Build: `cargo component build --release` (toolchain + `wasm-tools` per your org setup).
3. Copy the artifact over the fixture:

   ```bash
   cp target/wasm32-wasip1/release/schema_learner.wasm \
     rsworkspace/crates/mcp-pack/tests/fixtures/schema-learner-stub.wasm
   ```

4. Run `cargo test -p mcp-pack` to refresh manifest content hashes.

## Sign and publish workflow

1. **Build** — `McpPack::build()` or `cargo run -p mcp-pack -- build --out pack.bundle`.
2. **Trust** — Add `[signing].nkey_pub` to gateway `trusted_signers` / `trusted_signers_first_party` (proposed).
3. **Verify offline** — `trogon_mcp_gateway::bundle::verify_bundle(&bytes, &trusted)` (or future `agctl mcp bundle validate`).
4. **Publish** — Push tarball to OCI (`mcp-policy@<semver>+<sha256-12>`) and/or NATS KV `mcp-policy-bundles` key `global/mcp-pack` (ADR 0028). Pin **digest** in `mcp-gateway-config` for hot-swap.

Never edit a published artifact in place; roll forward with a new semver and digest.
