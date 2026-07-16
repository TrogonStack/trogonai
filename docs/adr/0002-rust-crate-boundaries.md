---
number: "0002"
slug: rust-crate-boundaries
status: accepted
date: 2026-06-08
---

# ADR#0002: Rust Crate Boundaries

## Context

The Rust workspace needs stable rules for package names, crate granularity, and
feature flags. Without those rules, the workspace can drift toward two bad
shapes: tiny crates for every concept, or large crates that mix domain,
transport, runtime, test, and operational concerns behind feature flags.

Cargo terminology matters here. A package is the `Cargo.toml` unit. A package
contains one or more crate targets: at most one library crate and any number of
binary crates. The Rust workspace is a set of related packages that share Cargo
workspace configuration.

## Decision

Default to one cohesive package per stable responsibility. Use Rust modules and
files inside a package before creating another package. Split into another
package only when there is a real boundary that another package should depend on.

Do not create a mini-crate for every type, helper, trait, adapter method, or
value object. Value objects belong in their own files inside the owning crate.
The package boundary is for dependency ownership, not file organization.

Do not use one large package to hide unrelated responsibilities behind feature
flags. Feature flags are for optional capabilities and optional dependencies, not
for replacing package boundaries.

## Naming Rules

Use kebab-case package names in `Cargo.toml`. The Rust crate name used from code
is the package name with hyphens treated as underscores by Rust.

Name the package by the API or runtime role it gives to dependents:

```text
<domain>
<domain>-<capability>
<domain>-<adapter>
<domain>-<runtime>
<protocol>-<transport>
<protocol>-<transport>-<role>
```

Use the existing workspace prefixes consistently:

- `trogon-*` for first-party Trogon platform crates whose public contract does
  not depend on current TrogonAI repo or product assumptions. These crates should
  be plausible candidates for extraction into standalone libraries.
- `trogonai-*` for crates whose public contract belongs to this repo or product
  as it exists today, including product-facing APIs, generated types, schemas,
  workflows, deployment assumptions, and repo-specific integrations.
- `mcp-*` and `acp-*` when the external protocol identity is the primary
  boundary.

When the boundary is ambiguous, default to `trogonai-*` until the crate has a
clear extraction boundary. Dependency direction should normally flow from
`trogonai-*` crates to `trogon-*` crates, not from `trogon-*` crates back into
repo-specific `trogonai-*` crates.

Use role suffixes only when they describe a real boundary:

- `-runtime` for execution/runtime machinery.
- `-nats`, `-stdio`, or another transport suffix for transport adapters.
- `-proto` for generated or protocol-owned types.
- `-telemetry` for shared observability setup.
- `-config` for configuration value objects and loaders.
- `-client` for client APIs used across boundaries.
- `-sdk` for an external developer-facing toolkit that composes multiple client
  APIs, generated types, auth/config, and ergonomic helpers behind a stable
  product contract.
- `-macros` for a procedural macro package with function-like or attribute
  macros.
- `-derive` for a procedural macro package whose public role is derive macros.
- `-server` only for a package whose public role is serving a protocol; if it is
  production-operated, place it according to the service rules in [ADR#0001](./0001-workspace-runtime-taxonomy.md).

Avoid vague names such as `common`, `utils`, `helpers`, `core`, and `shared`.
Those names usually mean the actual boundary has not been found yet. Prefer a
domain or role name that explains why another package should depend on it.

## SDK Naming Rules

Do not use `-sdk` as a synonym for `-client`.

Use `-client` when the package is a focused client for one service, protocol, or
API boundary. Use `-sdk` only when the package is an intentionally
developer-facing toolkit for building against a product, platform, or protocol
role.

An SDK package should normally have most of these traits:

- It is intended for external or cross-team application developers.
- It has a stable product-facing compatibility contract.
- It composes multiple generated clients, protocol clients, or service clients.
- It owns developer ergonomics such as auth setup, endpoint configuration,
  retries, pagination, streaming helpers, or higher-level workflows.
- It has user-facing documentation and examples.
- It can hide internal service topology behind a product API.

Protocol role SDKs are allowed even when they focus on one protocol role. A
protocol role SDK gives application developers the callback traits, builders,
runtime glue, type conversions, and test harnesses needed to implement one side
of a protocol.

Use this pattern for protocol role SDKs:

```text
<protocol>-<role>-sdk
<protocol>-<backbone>-<role>-sdk
```

Examples:

- `mcp-server-sdk` for implementing MCP server callbacks.
- `mcp-client-sdk` for implementing MCP client callbacks.
- `acp-agent-sdk` for implementing ACP agent callbacks.
- `acp-client-sdk` for implementing ACP client callbacks.
- `acp-nats-agent-sdk` only when the SDK is intentionally NATS-specific.

Use protocol role names from the protocol, not deployment words. In an SDK name,
`server` is acceptable when it means the protocol role, such as MCP server. It
must not mean "this package is a deployable service."

Prefer a transport-agnostic role SDK when callbacks are independent of the
transport. Put transport-specific connection handling in adapter crates such as
`<protocol>-<transport>` or `<protocol>-<transport>-<role>`. Use a
transport-specific SDK name only when the developer-facing callback model itself
depends on that transport.

Do not use `-sdk` for:

- a single generated client
- a thin wrapper over one API
- internal-only helper crates
- protocol model crates
- service implementation packages
- transport adapters
- workspace automation

Split client-side and server-side SDKs when the callback contracts, dependencies,
runtime assumptions, or implementer audiences differ. A developer implementing an
MCP server should not have to depend on an MCP client SDK unless both roles are
part of one intentional developer toolkit.

Use one `<protocol>-sdk` package only when the client-side and server-side SDKs
are always versioned together, share the same dependencies, are commonly used
together, and have one coherent developer-facing documentation surface.

Prefer names like `trogonai-client`, `mcp-nats-client`, or
`trogon-scheduler-client` for focused clients. Reserve `trogonai-sdk` for a
product-level Rust SDK that intentionally brings together multiple capabilities
behind one developer-facing API.

For non-Rust SDKs, place the package according to the language ecosystem and
monorepo rules rather than forcing it into `rsworkspace/`. For example, a
TypeScript SDK may belong under a top-level SDK or app boundary with its own
package manager configuration. The `-sdk` suffix still requires a real
developer-facing product contract.

## When To Split

Create a new package when at least one of these is true:

- A public API is reused by multiple packages.
- The dependency set is meaningfully different and should not be imposed on the
  current package.
- A transport, storage backend, protocol integration, or runtime can change
  independently from the domain API.
- A generated protocol surface needs its own dependency and regeneration
  boundary.
- Procedural macros are needed. They require a `proc-macro` crate type and must
  live in a procedural macro package.
- Test support needs to be shared across packages through a `test-support`
  feature.
- The package has a different publication, ownership, or compatibility contract.
- The code would otherwise require feature flags that change the meaning of the
  same public API.

Keep code in the same package when most of these are true:

- It has one owner and one reason to change.
- It is private implementation detail.
- Splitting would only move modules into separate manifests.
- Dependents would almost always need both packages together.
- The split would create circular design pressure.
- The only reason for the split is file size.

Large cohesive crates are acceptable. Small crates are acceptable when they
represent real dependency boundaries. The goal is not crate count; the goal is
stable ownership and clear dependency direction.

## Crates Versus Features

Features are capabilities. Crates are boundaries.

Prefer one crate with features when all of these are true:

- The enabled code is still part of the same public API boundary.
- Every feature combination is additive and can be enabled together.
- The feature only controls optional dependencies, optional integrations, test
  support, generated protocol families, or environment-specific capabilities.
- Enabling the feature does not turn the crate into a different role, runtime,
  product surface, or operational unit.

Prefer multiple crates when any of these are true:

- The options represent different roles, such as agent, server, stdio bridge,
  CLI, worker, or service.
- The options impose substantially different dependency sets.
- The options have different runtime behavior, binaries, configuration,
  deployment, telemetry identity, or operational ownership.
- The options should be depended on independently by different callers.
- The options would require mutually exclusive features or feature combinations
  that cannot safely be enabled together.
- Enabling all features would produce a bloated or misleading public API.

Use one package with multiple binary targets when the binaries are still one
cohesive application boundary, share ownership and release cadence, have similar
dependencies, and are not intended to be depended on independently as libraries.
Do not split only because there are two executable entrypoints.

Cargo unifies features for a package across the dependency graph. Because of
that, feature flags cannot safely model mutually exclusive roles or ownership
boundaries. If two callers need different roles from the same package, Cargo may
build that package with the union of the requested features. That is correct for
additive capabilities and wrong for role selection.

For protocol or transport families, prefer this shape when there is both shared
library code and role-specific implementations:

```text
<protocol>-<transport>          shared protocol/transport API
<protocol>-<transport>-<role>   role-specific library, binary, or runtime
```

## Multiple Transports

Transport support has three separate questions:

- Is the transport dependency present at compile time?
- Is the transport enabled by runtime configuration?
- Does the transport represent a separate package, runtime, or ownership
  boundary?

Do not answer all three questions with Cargo features.

Use runtime configuration for transports that one service, app, or CLI can serve
at the same time. Configuration should use typed values, such as enums or
collections of transport variants, rather than boolean flags per transport.

Use Cargo features only when a transport is an additive optional capability
inside the same public API boundary. Every enabled transport feature must be able
to coexist with every other enabled transport feature.

Use separate transport adapter packages when the transport has a distinct
dependency set, API surface, runtime model, test strategy, or independent
dependents. The shared protocol or domain package should remain transport
agnostic, and the service/app/CLI composes the transport adapters it needs.

Use one package with multiple transports when they form one cohesive runtime
surface. For example, one remote transport listener may expose HTTP, SSE, and
WebSocket together when they share connection management, configuration,
telemetry identity, deployment, and ownership.

Use separate packages or binaries when transports imply different execution
models. A local stdio bridge and a network listener may share protocol code, but
they often differ in dependencies, startup, shutdown, configuration, telemetry
identity, and operational ownership.

Do not create empty transport adapter packages before the first real
implementation exists.

The ACP over NATS packages currently follow this model:

- `acp-nats` owns the shared ACP-over-NATS protocol and transport boundary.
- `acp-nats-agent` owns the agent-side role.
- `acp-nats-server` is the remote ACP transport listener for HTTP, SSE, and
  WebSocket clients.
- `acp-nats-stdio` is the local stdio bridge for IDEs and CLI clients.

Do not collapse those roles into `acp-nats` features such as `agent`, `server`,
and `stdio`. Those roles have different dependency surfaces and runtime
meanings. They should remain separate crates that depend on the shared
`acp-nats` boundary.

The `server` suffix is descriptive only when the package's public role is to
serve a protocol over a network transport. Prefer a more specific transport name
when it would make the boundary clearer, such as `-http`, `-websocket`,
`-streamable-http`, or `-remote`. Do not use `server` as a generic suffix for
any production runtime.

`acp-nats-server` and `acp-nats-stdio` could be one package with multiple
binaries if the intended boundary were "ACP client bridge over NATS" and the
combined dependency set, release cadence, and operational ownership were
acceptable. Keep them separate when HTTP/WebSocket serving and stdio bridging are
separate dependency, runtime, configuration, or telemetry surfaces.

When creating a new family, start with the shared crate only if that is the only
real boundary. Add role crates when the first role-specific implementation
appears. Do not create empty sibling crates for roles that do not exist yet.

## Feature Flag Rules

Features must be additive. Enabling a feature must not remove behavior or change
the meaning of an existing public API.

Use features for:

- Optional dependencies.
- Optional integrations.
- Test support shared across packages.
- Expensive or environment-specific capabilities.
- Generated protocol families that are not always needed.

Do not use features for:

- Mutually exclusive product modes.
- Hiding unrelated responsibilities inside one package.
- Switching domain semantics.
- Choosing between production implementations that should be separate adapters.
- Making invalid dependency direction appear valid.

Default features should stay empty unless there is a clear ergonomic reason for
the common case. If a dependency is optional, expose it through an intentionally
named feature instead of leaking the dependency name as the public concept.

Use `test-support` for shared test helpers, matching the Rust workspace
instructions. Keep `test-support` out of default features.

Do not create a procedural macro package because a normal `macro_rules!` macro
exists. Keep declarative macros in the owning crate until a real dependency
boundary appears. Create a `-macros` or `-derive` package only for procedural
macros or when macro dependencies would otherwise leak into ordinary runtime
code.

Every non-obvious feature must be documented in the package README or crate
docs. Feature combinations that matter must be covered by CI. At minimum,
packages with features should build and test with default features, no default
features, and all features when those combinations are meaningful.

## Dependency Rules

Dependency direction should move from specific runtime concerns toward reusable
domain and protocol code:

```text
services / apps / cli
  -> adapters / clients
  -> runtime
  -> domain
  -> std / protocol / value objects
```

Adapters may depend on domain crates. Domain crates must not depend on adapters,
services, apps, or CLIs.

Generated protocol crates should avoid depending on runtime packages unless the
feature name makes that coupling explicit and the dependency is optional.

## Examples

Existing names that follow the intended shape:

- `trogon-decider`: domain API for decider primitives.
- `trogon-decider-runtime`: runtime execution machinery for deciders.
- `trogon-decider-nats`: NATS adapter for decider runtime concerns.
- `trogon-nats`: shared NATS infrastructure boundary.
- `trogon-telemetry`: shared observability setup.
- `trogonai-proto`: generated or protocol-owned TrogonAI types.
- `mcp-nats` and `acp-nats`: protocol plus transport boundaries.

When a crate starts as a single cohesive package, keep it that way until a
dependent package, dependency set, or operational boundary proves that a split is
needed.

## Consequences

This keeps the workspace from optimizing for either minimal manifests or maximal
decomposition. Packages are introduced when they improve dependency ownership,
compile surface, optional dependency control, or API stability.

Feature flags remain a small, additive capability mechanism rather than a hidden
architecture system.

## References

- [The Rust Book: packages and crates](https://doc.rust-lang.org/stable/book/ch07-01-packages-and-crates.html)
- [The Rust Book: Cargo workspaces](https://doc.rust-lang.org/book/ch14-03-cargo-workspaces.html)
- [The Cargo Book: features](https://doc.rust-lang.org/cargo/reference/features.html)
- [The Cargo Book: workspaces](https://doc.rust-lang.org/stable/cargo/reference/workspaces.html)
- [The Rust Reference: procedural macros](https://doc.rust-lang.org/reference/procedural-macros.html)
- [The Rust Reference: macros by example](https://doc.rust-lang.org/reference/macros-by-example.html)
