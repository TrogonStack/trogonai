---
number: "0005"
slug: polyglot-workspace-layout
status: accepted
date: 2026-06-08
---

# ADR 0005: Polyglot Workspace Layout

## Context

The repository will contain more than one language ecosystem. Rust already has
`rsworkspace/`. TypeScript packages and example Next.js applications are
expected. Go or Python may be added later.

The workspace layout should feel consistent across ecosystems without erasing
important language-specific conventions. In particular, Rust should keep its
Cargo-specific crate rules from [ADR 0002](./0002-rust-crate-boundaries.md).

## Decision

Use one shared architectural vocabulary across language workspaces:

| Concept | Meaning |
| --- | --- |
| Reusable package | Library code intended to be imported by other packages. |
| Service | Production-operated workload with runtime ownership and telemetry identity. |
| App | Product-facing application surface. |
| CLI | Command-line application surface. |
| SDK | Developer-facing toolkit with a stable product/platform/protocol-role contract. |
| Example | Non-production sample that demonstrates usage. |
| Workspace automation | Internal commands used to build, test, generate, or maintain the workspace. |

Each language workspace maps those concepts into the idioms of that ecosystem.
Do not force every language to use the same folder word when the ecosystem has a
clearer convention.

## Default Workspace Names

Use these top-level workspace names when the ecosystem becomes large enough to
need a dedicated workspace:

```text
rsworkspace/     Rust / Cargo workspace
tsworkspace/     TypeScript and JavaScript workspace
goworkspace/     Go workspace, if needed
pyworkspace/     Python workspace, if needed
```

Do not create a language workspace before it has more than one real package or a
real toolchain reason to exist. A single example or one-off package may live in
the most direct location until a workspace boundary is justified.

## Shared Shape

Language workspaces should follow this shape where applicable:

```text
<language-workspace>/
  packages-or-crates/
  services/
  apps/
  cli/
  sdks/
  examples/
  automation/
```

Only create directories that contain real packages. Do not add empty buckets for
future intent.

## Rust Mapping

Rust keeps Cargo terminology:

```text
rsworkspace/
  crates/       reusable Rust packages
  services/     Rust production workloads, when introduced
  apps/         Rust-first app boundaries, when grouping is justified
  cli/          product-facing Rust CLIs, when needed
  sdks/         Rust SDK packages, when they are real SDK boundaries
  examples/     Rust examples or sample apps
  xtask/        Rust workspace automation, if a dedicated automation crate exists
```

Rust reusable packages use `crates/`, not `packages/`. [ADR 0002](./0002-rust-crate-boundaries.md) remains the
source of truth for Rust crate naming, crate granularity, feature flags,
transport adapter packages, SDK names, and procedural macro packages.

## TypeScript Mapping

Use `tsworkspace/` when TypeScript grows beyond isolated docs or examples.

```text
tsworkspace/
  packages/     reusable TypeScript packages
  services/     TypeScript production workloads
  apps/         product-facing web, desktop, or hosted applications
  cli/          product-facing TypeScript CLIs
  sdks/         TypeScript SDK packages
  examples/     sample apps, including Next.js examples
  automation/   workspace automation packages, only when package scripts are not enough
```

TypeScript package names should follow package-manager conventions, normally
scoped names such as `@trogonai/<name>` when publishing or cross-package imports
are expected.

Use `packages/` for reusable TypeScript libraries because that is the ecosystem
convention. Do not use `crates/` outside Rust.

Next.js belongs under `apps/` only when it is a product-facing application.
Example Next.js applications belong under `examples/`.

## App Surface Rules

Do not create top-level `webapps/`, `mobileapps/`, or `desktopapps/`
directories by default. Web, mobile, and desktop describe platform surfaces of
an app; they are not separate architectural ownership categories.

If a product has one application surface, keep the app at the normal app path
and let the package metadata, framework, and README describe the platform:

```text
tsworkspace/apps/<app-name>/
```

If one product has multiple platform surfaces that share product ownership,
domain model, release planning, and roadmap, group them under the product app
boundary:

```text
tsworkspace/apps/<product-name>/
  web/
  mobile/
  desktop/
  shared/
```

Only create the surface directories that exist. Do not add `web/`, `mobile/`, or
`desktop/` as placeholders for future intent.

If the surfaces have independent product ownership, release cadence, roadmap, or
operational responsibility, model them as separate app packages instead:

```text
tsworkspace/apps/<web-app-name>/
tsworkspace/apps/<mobile-app-name>/
tsworkspace/apps/<desktop-app-name>/
```

Examples follow the same rule. A single-surface example stays directly under
`examples/`. A multi-surface example may group surfaces under one example
boundary:

```text
tsworkspace/examples/<example-name>/
  web/
  mobile/
  desktop/
```

Native mobile or desktop ecosystems may justify their own language or toolchain
workspace later, but the naming should still preserve the product boundary. Do
not introduce `mobileapps/` or `desktopapps/` only because a framework such as
React Native, Tauri, Electron, Swift, Kotlin, or Flutter appears.

## Go Mapping

Use `goworkspace/` only when Go has multiple modules/packages or a real
toolchain reason to use a Go workspace.

```text
goworkspace/
  packages/     reusable Go packages/modules when they are imported across boundaries
  services/     Go production workloads
  apps/         Go product-facing application boundaries, if any
  cli/          Go CLIs
  sdks/         Go SDK modules
  examples/     Go examples
```

Follow Go module/package conventions inside this workspace. Do not create nested
modules by default; use them only when versioning, dependency isolation, or
publication requires it.

## Python Mapping

Use `pyworkspace/` only when Python has multiple packages, examples, or a real
toolchain reason to centralize Python configuration.

```text
pyworkspace/
  packages/     reusable Python packages
  services/     Python production workloads
  apps/         Python product-facing application boundaries, if any
  cli/          Python CLIs
  sdks/         Python SDK packages
  examples/     Python examples
```

Follow Python packaging conventions inside this workspace. Do not split Python
packages only to mirror Rust crate count.

## Examples

Examples are not apps by default.

Put sample code under `examples/` when its purpose is to demonstrate usage,
exercise an SDK, validate an integration, or provide a starting point for users.

Move an example to `apps/` only when it becomes a product-facing application with
real ownership, release cadence, operational expectations, or user-facing
roadmap.

A Rust-backed Next.js example should normally live under:

```text
tsworkspace/examples/<example-name>/
```

If the example needs Rust packages, those Rust packages stay in `rsworkspace/`
and the example consumes the published, generated, or locally linked interface.
Do not place a Next.js project inside `rsworkspace/` only because it demonstrates
Rust functionality.

## Cross-Language Boundaries

Cross-language packages should communicate through explicit contracts:

- Protocol Buffers for first-party machine-to-machine contracts.
- NATS-backed messages for internal runtime paths, when possible.
- ConnectRPC for first-party service APIs after an API surface is necessary.
- Protocol role SDKs for MCP/ACP callback surfaces.
- Generated clients or generated protocol packages.
- Stable CLI or process boundaries when appropriate.

Do not import across language workspaces through ad hoc file paths. If one
language needs artifacts from another, use generated code, package manager links,
published packages, or a documented build step.

## Dependency Direction

Each language workspace follows the same dependency direction:

```text
apps / services / cli / examples
  -> sdks / clients / adapters
  -> reusable packages
  -> generated protocol packages / value objects
```

Examples may depend on SDKs, clients, adapters, and reusable packages. Reusable
packages must not depend on examples.

Services and apps should not depend directly on another service implementation.
Use protocol/client/SDK packages instead.

## Consequences

The monorepo gets consistent concepts across languages:

- Rust uses `crates/` because Cargo and [ADR 0002](./0002-rust-crate-boundaries.md) are Rust-specific.
- TypeScript, Go, and Python use `packages/` for reusable libraries.
- `apps/`, `services/`, `cli/`, `sdks/`, and `examples/` mean the same thing in
  every language workspace.
- Example applications do not become product apps by accident.
- New language workspaces are created when they solve a real ecosystem or scale
  problem, not preemptively.

## References

- [ADR 0001: Workspace Runtime Taxonomy](./0001-workspace-runtime-taxonomy.md)
- [ADR 0002: Rust Crate Boundaries](./0002-rust-crate-boundaries.md)
- [ADR 0003: AI Protocol Transport Taxonomy](./0003-ai-protocol-transport-taxonomy.md)
- [ADR 0004: Protocol and Transport Layering](./0004-protocol-and-transport-layering.md)
