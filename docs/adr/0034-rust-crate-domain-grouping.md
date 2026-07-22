---
number: "0034"
slug: rust-crate-domain-grouping
status: accepted
date: 2026-07-21
---

# ADR#0034: Rust Crate Domain Grouping

## Context

The Rust workspace grew to 44 crates directly under `rsworkspace/crates/`.
[ADR#0002](./0002-rust-crate-boundaries.md) governs when a crate exists and how
it is named; [ADR#0005](./0005-polyglot-workspace-layout.md) fixes `crates/` as
the home for reusable Rust packages. Neither says how crates are arranged
*inside* `crates/`, and a single flat directory of 44 entries makes the domain
families (a2a, acp, mcp, ard, decider, aauth, scheduler) hard to see at a glance
and gives newcomers no structural signal about which crates belong together.

The crate name prefixes already encode those families, so the grouping exists
conceptually; it is just not reflected on disk.

## Decision

Group crates under `rsworkspace/crates/<group>/<crate>`, one directory level per
domain family. Crate package names are unchanged; only filesystem paths move.

The workspace uses a two-level member glob:

```toml
members = ["crates/*/*", "wasm-components/*", "cli/trogon-decider-test"]
```

A single-level `crates/*` glob cannot coexist with grouped crates: Cargo treats
every glob match as a workspace member and fails to load a group directory that
has no `Cargo.toml`. Every crate therefore lives exactly one level deep; there
are no crates directly under `crates/`.

Current groups:

| Group | Purpose |
| --- | --- |
| `a2a/` | A2A protocol, transports, gateway, and supporting crates. |
| `acp/` | ACP-over-NATS protocol and role crates. |
| `mcp/` | MCP-over-NATS protocol and role crates. |
| `ard/` | Agent discovery catalog and registry. |
| `decider/` | Decider domain, runtime, WASM host, guest SDK, and sim. |
| `aauth/` | Agent authentication crates. |
| `scheduler/` | Scheduler domain and service. |
| `platform/` | Cross-cutting foundations and standalone services that are not part of a domain family (e.g. `trogon-std`, `trogon-nats`, `trogon-telemetry`, `trogonai-proto`, `trogon-gateway`). |

### Grouping rule

A group directory exists when two or more crates share a domain. Any crate that
is not part of a multi-crate domain family lives in `platform/`.

`platform/` is a directory grouping label, not a crate name. [ADR#0002](./0002-rust-crate-boundaries.md)
bans vague *package* names such as `core`, `common`, `shared`, and `utils`; that
rule is unchanged and no crate may be named after its group. The label is
`platform/` rather than `core/` to stay clear of that banned vocabulary while
matching how ADR#0002 already describes `trogon-*` as first-party platform
crates.

### What does not change

- Crate package names, and therefore every `{ workspace = true }` dependency.
- The role-partitioned top-level directories `cli/` and `wasm-components/`
  from [ADR#0001](./0001-workspace-runtime-taxonomy.md); grouping applies inside
  `crates/` only.
- The dependency-direction rules in [ADR#0002](./0002-rust-crate-boundaries.md).
  A group directory carries no dependency meaning; it is navigation only.

## Consequences

Domain families are visible on disk and new crates have an obvious home. The
one-time cost is a filesystem move plus updates to every path-based reference:
the workspace member glob, internal `path = "..."` dependencies, per-crate
`target/` gitignore depth, coverage ignore globs, release automation paths, and
any tooling that addresses a crate by filesystem path rather than package name.
Moves use `git mv` so history follows.

Adding a crate to an existing family is a drop-in. Introducing a new family is a
new directory plus its member coverage under the existing `crates/*/*` glob. A
crate that outgrows `platform/` into its own family moves with `git mv` and a
path-reference sweep, the same cost as any rename.

## References

- [ADR#0001: Workspace Runtime Taxonomy](./0001-workspace-runtime-taxonomy.md)
- [ADR#0002: Rust Crate Boundaries](./0002-rust-crate-boundaries.md)
- [ADR#0005: Polyglot Workspace Layout](./0005-polyglot-workspace-layout.md)
- [The Cargo Book: workspaces](https://doc.rust-lang.org/stable/cargo/reference/workspaces.html)
