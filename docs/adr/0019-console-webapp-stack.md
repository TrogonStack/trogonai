---
number: "0019"
slug: console-webapp-stack
status: accepted
date: 2026-07-08
---

# ADR 0019: Console Webapp Stack

## Context

The platform needs an operator console: a product-facing web application for
managing agents and schedules, browsing the discovery catalog
([ADR 0012](./0012-ard-compatible-discovery-catalog.md)), and observing live
execution. It is the repository's first product web application, so its stack
choices set the default for every later TypeScript product surface.

The console's data path is decided by
[ADR 0018](./0018-connectrpc-gateway-for-browser-product-surfaces.md): the
browser speaks ConnectRPC to a gateway that bridges onto the NATS backbone and
owns all credentials. This ADR decides everything above that boundary: the
application framework, build tooling, component system, data and form
libraries, testing, and how the application scales to more product areas
without a runtime composition scheme.

An operator console is an authenticated, information-dense internal surface.
It has no anonymous visitors, no search-engine surface, and no
first-paint-for-logged-out-users requirement.

## Decision

### 1. Placement follows ADR 0005

The console lives at `tsworkspace/apps/console/`, with reusable TypeScript
packages under `tsworkspace/packages/`, per the layout and App Surface Rules
in [ADR 0005](./0005-polyglot-workspace-layout.md). There is no top-level
`webapps/` directory. If the console later grows desktop or mobile surfaces,
it regroups as `tsworkspace/apps/console/{web,desktop,shared}/` as ADR 0005
already prescribes. The workspace uses pnpm, with Turborepo as the task
runner so build, test, and typecheck tasks are dependency-ordered and cached
as the package graph grows.

Package names use the `@trogonai/<name>` scope per ADR 0005, and the scope is
protected against public-registry collision deliberately. An npm scope is an
owned namespace, so the `@trogonai` organization must be registered on the
public npm registry (unclaimed and empty as of 2026-07-08) before these names
are treated as authoritative; once claimed, no third party can publish under
them. Independently of scope ownership, internal cross-package dependencies
use the pnpm `workspace:` protocol, which resolves only inside the workspace
and never falls back to the registry, closing the dependency-confusion path
even for an unclaimed name, and every package not deliberately published
carries `"private": true`. Directory-derived scopes such as `@tsworkspace/*`
are not used: the scope names the owning identity, not the repository layout,
and it must remain a namespace the project can actually own on the registry.

### 2. The console is a Vite single-page application, not a meta-framework app

The console is a client-rendered SPA built with Vite. Next.js, Nuxt, and
similar server-rendering meta-frameworks are not used. Their value is a
server tier for SEO and anonymous first paint, which this surface does not
have, and that tier would duplicate session and credential responsibilities
that ADR 0018 assigns to the gateway. The build output is static assets served
by the console gateway, keeping the product surface one deployable workload.

If server rendering ever becomes a real requirement, the migration path is
TanStack Start, which builds on the same router and Vite toolchain.

### 3. React with a TanStack-first library rule

The console is a React application. When a needed capability has a viable
TanStack option, the console uses it before considering alternatives:

- Routing: TanStack Router with file-based route trees.
- Server state: TanStack Query over the generated Connect clients. Query owns
  every server-derived value; no server data is copied into client stores.
- Tables: TanStack Table, with TanStack Virtual when row counts require
  virtualization.
- Forms: TanStack Form. Form owns edit state; submission goes through a
  Connect mutation and Query invalidation.
- Client-only state: component-local state by default. If shared client state
  accumulates, TanStack Store before any other state library.

Deviating from a viable TanStack option requires a stated reason in the code
review that introduces the deviation.

### 4. UI components are shadcn on Base UI, and Radix is prohibited

Styling is Tailwind CSS. Components follow the shadcn model (source-owned
components generated into the repository, not a packaged component
dependency) built on Base UI primitives. Exactly one headless primitive layer
is allowed in the dependency tree: `@radix-ui/*` packages must not appear,
directly or transitively. Community shadcn components that import Radix are
ported to Base UI rather than adopted as-is. The workspace enforces the ban
with tooling rather than review vigilance.

### 5. Validation is zod at the boundary, per the ADR 0009 conversion rule

Runtime validation uses zod, wired into TanStack Form through Standard
Schema. Generated protobuf types do not leak primitive obsession into
application code: screens and forms convert generated messages into richer
edit models at the boundary when validation, units, identity, or invariants
matter, the same conversion rule
[ADR 0009](./0009-protocol-buffers-wire-contracts.md) sets for domain code.

### 6. Generated Connect clients come from the existing Buf pipeline

`protobuf-es` and `connect-es` code generation is added to the repository's
`buf.gen.yaml`, emitting a generated protocol package under
`tsworkspace/packages/`. The `.proto` sources under `proto/` remain the only
wire contract; the console defines no hand-written request or response
shapes.

### 7. Testing is unit-first, per ADR 0010's posture

- Vitest with Testing Library for unit and component tests.
- Connect's router transport for API mocking, so tests exercise the same
  generated service interfaces the app uses instead of intercepting HTTP.
- Playwright for end-to-end coverage of the flows that cannot be trusted to
  unit tests, primarily authentication and live streaming.

### 8. Lint and format run on the Oxc toolchain

Linting is oxlint and formatting is oxfmt, run workspace-wide, replacing the
ESLint + Prettier combination. One Rust toolchain covers both jobs with near
instant runs, which matches the repository's tooling posture, and oxlint's
default rule set applies without a config sprawl. Generated files
(`routeTree.gen.ts`) are excluded from formatting. Adopting ESLint later is
warranted only if a needed rule class (for example exhaustive React hooks
checks) is missing from oxlint, and that adoption is additive rather than a
replacement.

### 9. Telemetry follows ADR 0008

The console uses the OpenTelemetry Web SDK, exports OTLP to the same
collector path the platform uses, and propagates `traceparent` on every
Connect call through a client interceptor so one operator action traces from
the browser through the gateway onto the backbone
([ADR 0008](./0008-opentelemetry-observability.md)).

### 10. The console scales by vertical slices, not microfrontends

Each product area (agents, schedules, catalog, execution) is a vertical
slice: its own package or route subtree, lazily loaded through the router,
with screens, queries, and forms owned by the slice. Slices do not import
each other's internals; contact happens through shared packages or route
navigation.

Runtime composition schemes (module federation, microfrontends) are not
used. They solve independent deploy cadence across many teams, which this
surface does not have, at the permanent cost of duplicated dependencies,
singleton version skew, and design drift. The reopening conditions are:

- Multiple teams needing independent deploy cadence into one surface, which
  would justify revisiting composition.
- Untrusted third-party UI, which gets sandboxed iframes with a message
  bridge, not module federation, because the problem is isolation.

A different audience, trust level, or release cadence is a new app under
`tsworkspace/apps/` with its own gateway workload (ADR 0018), sharing code
through workspace packages at build time, never through runtime federation.

## Consequences

- The first product surface establishes `tsworkspace/` with `apps/console/`
  and the shared `packages/` split, so later TypeScript surfaces inherit a
  working layout instead of a greenfield decision.
- End-to-end typing holds from `.proto` to screen: generated Connect clients,
  typed routes, typed forms, and zod-validated edit models.
- The Radix prohibition keeps one primitive layer's focus, portal, and
  dismissal semantics across the console, at the cost of occasionally porting
  a community component to Base UI.
- The TanStack-first rule removes a per-feature library debate and keeps one
  vendor's conventions across routing, data, tables, and forms.
- Chart tooling is deliberately not pinned here; it is a reversible choice
  to make when the first chart lands.
- A future schema-driven UI (rendering service-declared config and status
  surfaces from the ADR 0012 discovery catalog) is the preferred path for
  third-party extensibility, keeping extension a matter of publishing
  schemas rather than shipping JavaScript into the console.

## References

- [ADR 0005: Polyglot Workspace Layout](./0005-polyglot-workspace-layout.md)
- [ADR 0008: OpenTelemetry Observability](./0008-opentelemetry-observability.md)
- [ADR 0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [ADR 0010: Unit Tests First, Testcontainers Only When Necessary](./0010-testcontainers-for-infrastructure-tests.md)
- [ADR 0012: ARD-Compatible Discovery Catalog](./0012-ard-compatible-discovery-catalog.md)
- [ADR 0018: ConnectRPC Gateway for Browser Product Surfaces](./0018-connectrpc-gateway-for-browser-product-surfaces.md)
- [TanStack](https://tanstack.com/)
- [Base UI](https://base-ui.com/)
- [shadcn/ui](https://ui.shadcn.com/)
