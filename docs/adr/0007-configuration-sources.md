---
number: "0007"
slug: configuration-sources
status: accepted
date: 2026-06-08
---

# ADR 0007: Configuration Sources

## Context

The repository needs a stable policy for how tools, services, CLIs, and
workspace automation receive configuration. Without one policy, each package can
choose different file formats, environment variable names, CLI flags, merge
rules, and override precedence.

Rust is the primary implementation ecosystem for the current workspace, and
TOML already fits Rust tooling and project conventions. `Cargo.toml` makes TOML
familiar to contributors, and Rust libraries can deserialize TOML into typed
configuration structures without treating every setting as an unstructured
string.

## Decision

Use TOML as the primary human-edited configuration file format for repository
tools, services, CLIs, and workspace automation when the implementation can
support it well.

Configuration sources must resolve in this precedence order:

| Precedence | Source | Purpose |
| --- | --- | --- |
| 1 | Built-in defaults | Stable fallback behavior owned by the implementation. |
| 2 | TOML config file | Shared project, package, service, or tool defaults. |
| 3 | Environment variables | Runtime, deployment, host, and secret-provided overrides. |
| 4 | CLI arguments | Explicit per-invocation overrides. |

Higher-precedence sources override lower-precedence sources. CLI arguments are
the most explicit signal for a single invocation and therefore win over every
other source.

## Format Rules

Use one primary config file format for each tool or service. Do not support TOML,
YAML, and JSON as equivalent first-class formats unless a real integration
requires it.

Prefer formats by role:

| Format | Use when |
| --- | --- |
| TOML | Humans edit the config, comments are useful, and Rust support is available. |
| JSON | Machines generate the config or an integration requires JSON schemas. |
| YAML | The surrounding ecosystem already expects YAML. |

Configuration files should not store secrets. Secrets should come from
environment variables, secret stores, or deployment/runtime infrastructure.

## Resolution Rules

Every configurable value should have one canonical typed name in code. Config
files, environment variables, and CLI arguments may expose that name through
source-appropriate spelling, but they must map back to the same canonical
setting.

Use these spelling conventions by source:

| Source | Example |
| --- | --- |
| Typed config field | `nats_url` |
| TOML key | `nats_url` |
| Environment variable | `TROGON_NATS_URL` |
| CLI argument | `--nats-url` |

Config loading should deserialize into typed configuration structures or value
objects at the boundary. Validation should happen after all sources have been
merged so errors describe the effective configuration, not only one input file.

Unknown keys in TOML config files should fail validation by default. Unknown
environment variables should be ignored unless a tool explicitly documents a
strict environment allowlist. Unknown CLI arguments should fail through the CLI
parser.

## Merge Rules

Scalar values replace lower-precedence values.

Lists replace lower-precedence lists unless a tool documents an additive flag or
key. Maps merge by key, and each key follows the same source precedence rules.

If a tool supports multiple config files, it must document their load order and
why more than one file is necessary. A single local project config file is the
default.

## Introspection

Tools and services with non-trivial configuration should expose a resolved
configuration view, such as:

```text
<tool> config dump --resolved
<tool> config explain
```

The resolved view should identify the effective value and its source. Secret
values must be redacted.

## Consequences

- New Rust-backed tools should default to TOML configuration.
- Environment variables remain the deployment and secret override mechanism.
- CLI arguments remain the highest-precedence per-run override mechanism.
- JSON and YAML are reserved for integrations that need them.
- Code review should reject ambiguous configuration precedence and undocumented
  merge behavior.
