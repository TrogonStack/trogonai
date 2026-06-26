# TrogonAi Telemetry Semantic Conventions

This directory is the cross-runtime source of truth for the telemetry TrogonAi
emits: metric names, span names, and attribute keys. It is processed by
[OpenTelemetry Weaver](https://github.com/open-telemetry/weaver) to validate
naming against policy and to generate per-language bindings.

It lives at `otel/semconv/`, a language-neutral contract at the repository root
alongside `proto/`, from which each runtime regenerates typed bindings. Rust bindings are generated
today; Go and TypeScript packages are intended to be generated from this same
registry as those services appear.

## Layout

```
otel/semconv/
  registry/          # the registry: manifest + the as-is inventory of today's telemetry
    manifest.yaml    # schema_url, stability, optional upstream dependency
    common.yaml      # shared, reused-OTel, resource, and log-field attributes
    scheduler.yaml   # trogon-scheduler
    acp.yaml         # acp-nats
    nats.yaml        # trogon-nats
    mcp.yaml         # mcp-nats
    a2a.yaml         # a2a-nats
    gateway.yaml     # trogon-gateway webhooks
    std.yaml         # trogon-std (HTTP server span)
  policies/          # Rego policies (metric + attribute naming)
    trogon_naming.rego
  templates/         # Weaver Forge codegen templates, one subtree per target
    registry/rust/   # Rust target (weaver.yaml + *.j2)
```

The generated `trogon-semconv` crate under `rsworkspace/crates/trogon-semconv/src/gen/`
is the initial scheduler seed and is **not** regenerated from this inventory. It
stays frozen until the migration step; do not edit it by hand.

## Commands

All commands run through `mise`:

- `mise run semconv:check` validates the registry and enforces the naming
  policies in `policies/`.
- `mise run semconv:generate` regenerates the Rust bindings, then `cargo fmt`s
  them.

CI runs `semconv:check` (via the `otel/weaver` image) on any change under
`otel/semconv/`. Generation is a developer step whose output is committed, exactly
like `proto:generate`.

## Scope and relationship to current code

This registry is an **as-is inventory**: it records the metric, span, and
attribute names every crate emits *today*, exactly as emitted, with no naming
convention applied yet. That includes bare attribute keys (`outcome`, `method`,
`session_id`), unnamed transport spans (`send` / `receive`), `{source}.webhook`
gateway spans, and the overloaded `method` key (ACP method vs HTTP method).
Reused standard OpenTelemetry attributes (`messaging.*`, `http.*`, `server.*`,
`error.type`) are documented locally so the registry stays self-contained.

Coverage today: `trogon-scheduler`, `acp-nats`, `trogon-nats`, `mcp-nats`,
`a2a-nats`, `trogon-gateway`, and `trogon-std` (7 metrics, ~55 spans, ~50
attributes).

Nothing here is wired into the running code. Enforcing a naming convention (such
as a `trogonai.` root), referencing upstream OTel attributes instead of
re-declaring them, and migrating call sites onto generated constants are all
deferred follow-ups. The `trogon-semconv` crate stays the initial seed and is
not regenerated from this inventory.

Some attribute-less spans (all `a2a.server.*`, several `acp.client.*`,
`twitter.crc`) produce advisory Weaver warnings; `weaver registry check` still
passes. They flag spans that carry no attributes today.

## Referencing upstream OpenTelemetry conventions

Several signals reuse standard OTel attributes (`messaging.*`, `server.*`,
`error.type`, ...). To reference rather than redefine them, add the upstream
registry as the manifest `dependencies` entry (see the commented block in
`registry/manifest.yaml`). That requires network access during `check`/
`generate`, so it is omitted from the initial self-contained scaffold.

## Adding a new signal

1. Add or extend a `*.yaml` file under `registry/` (define attributes in a
   `registry.*` `attribute_group`; reference them from `metric`/`span` groups
   with `ref:`).
2. Run `mise run semconv:check` and fix any policy violations.
3. Run `mise run semconv:generate` and commit the updated `gen/` output.
