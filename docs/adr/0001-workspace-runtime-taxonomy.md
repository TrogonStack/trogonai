---
number: "0001"
slug: workspace-runtime-taxonomy
status: accepted
date: 2026-06-08
---

# ADR 0001: Workspace Runtime Taxonomy

## Context

The workspace needs stable names for code packages, product applications, runtime
workloads, CLIs, and workspace automation. Without strict meanings, directories
such as `crates`, `services`, `apps`, and `cli` can become interchangeable
buckets for anything executable.

The taxonomy should align with industry language instead of inventing local
terms. OpenTelemetry defines an application as one or more services, and a
service as a component that can have multiple deployed instances. Runtime
platforms use similar but more operational terms: Kubernetes calls deployed
runtime objects workloads, and process-model platforms commonly describe
entrypoints as process types such as `web`, `worker`, or `clock`.

## Decision

Use these meanings across the repository:

| Term | Meaning | Repository location |
| --- | --- | --- |
| Crate | A Rust package or reusable Rust library boundary. | `rsworkspace/crates/` |
| Service | A production-operated runtime workload with its own lifecycle and OpenTelemetry service identity. | Future `rsworkspace/services/` or equivalent service-owned package path. |
| App | A product-facing application surface for users or external systems. An app may be composed from one or more services. | Future `apps/` or product-specific application path. |
| CLI | A command-line application. A CLI is an app when product-facing, workspace automation when internal, and a service only when deployed as a managed production workload. | `apps/`, `cli/`, `xtask`, or a named package path depending on ownership. |

`services/` must not mean "anything executable." A service is a production
runtime unit with operational ownership. It normally has most of these traits:

- It is started by deployment or runtime infrastructure.
- It has production configuration, secrets, logs, alerts, dashboards, or SLOs.
- It owns an OpenTelemetry `service.name`, and optionally a `service.namespace`.
- It may have many runtime instances identified by `service.instance.id`.
- Its failure affects product behavior.
- It is operated as an API, worker, daemon, scheduler, webhook receiver, MCP
  server, Job, or CronJob.

`apps/` must not mean "all deployable things." An app is the product-facing
application boundary. Examples include a web frontend, desktop app, mobile app,
hosted product UI, external integration app, or user-facing product CLI. An app
can call services, embed client code from crates, or include a thin backend for
delivery, but the app name describes the user-facing product surface rather than
the production process type.

## Dependency Rules

Code dependencies flow inward toward reusable crates:

```text
apps                  -> crates
apps                  -> service client/protocol crates
cli                   -> crates
cli                   -> service client/protocol crates
services              -> crates
workspace automation  -> crates
```

Do not make reusable crates depend on services, apps, CLIs, or workspace
automation. Do not make an app or CLI depend directly on a service
implementation package. Do not make one service depend directly on another
service's implementation. Use shared crates, protocol crates, or client crates
for cross-boundary communication.

## Placement Rules

Place a package by the boundary it owns:

- Put reusable Rust libraries and shared types in `rsworkspace/crates/`.
- Put production-operated APIs, workers, schedulers, daemons, managed jobs, and
  protocol servers in `services/` once the workspace introduces that directory.
- Put product-facing user applications in `apps/`.
- Put internal one-shot commands in an `xtask`-style package or a named crate.
- Put a user-facing CLI in `apps/`, `cli/`, or a named product package path.
- Keep a CLI outside `services/` unless deployment infrastructure operates that
  CLI as a production workload.

`xtask` is a Rust community convention for project-local automation, not a
specific required external tool. An `xtask`-style package usually contains
commands such as code generation, release preparation, repository checks, or
other developer workflows that benefit from using the same language, dependency
graph, and lints as the workspace.

Current crates do not need to move immediately. This decision defines the future
boundary language. Moving existing packages is a separate migration decision.

## Grouping Rules

Default to waiting. Do not create an application umbrella only because a product
may eventually need a CLI, services, and private crates. Start with the smallest
clear placement for the artifact that exists now.

Group from the beginning only when the application boundary is already real:

- The app has a clear product name or user-facing boundary.
- The first implementation already needs more than one artifact type, such as a
  CLI and service, or a service and private support crate.
- The artifacts share ownership, release cadence, domain language, and
  operational responsibility.
- The grouped crates are private to that application boundary.
- Other applications or services are not expected to import the grouped internals
  directly.

When those conditions hold, use the application as the umbrella and preserve
artifact roles inside it:

```text
apps/<app-name>/
  cli/
  services/
    <process-type>/
  crates/
    <private-crate>/
```

The grouping does not change the taxonomy. Each service inside the application
still owns its own OpenTelemetry `service.name`. The CLI remains a CLI. Private
crates remain implementation details of the application boundary.

Move code out of an application boundary when it becomes shared. Reusable crates
belong in `rsworkspace/crates/`, and cross-boundary communication should happen
through shared protocol or client crates rather than direct imports of grouped
service implementations.

## Monorepo and Workspace Rules

The monorepo is the ownership container. It can hold documentation, protocol
definitions, deployment configuration, Rust packages, web packages, generated
assets, and application boundaries.

A language workspace is a build and dependency mechanism inside the monorepo. It
is not the architectural boundary. In this repository, `rsworkspace/` is the Rust
workspace and `docs/` is a documentation workspace. The runtime taxonomy decides
what a package is; the language workspace decides how that package is built,
linted, tested, and versioned with its ecosystem.

Apply the taxonomy inside each workspace:

- Rust reusable packages live under `rsworkspace/crates/`.
- Rust services should live under `rsworkspace/services/` when that directory is
  introduced, then be added to `rsworkspace/Cargo.toml` workspace members.
- Rust product CLIs should live under `rsworkspace/cli/` or under a grouped
  application boundary when they are private to that app.
- Rust workspace automation should use an `xtask`-style package or named crate
  and participate in the Rust workspace when it benefits from shared lints,
  dependencies, and CI.
- Non-Rust apps may live under top-level `apps/<app-name>/` with their own
  ecosystem workspace files.

When an application boundary groups Rust artifacts, prefer keeping the Rust
packages inside the Rust workspace so they share Cargo lockfile, lints, and
workspace dependencies:

```text
rsworkspace/apps/<app-name>/
  cli/
  services/
    <process-type>/
  crates/
    <private-crate>/
```

Use a top-level `apps/<app-name>/` for polyglot product boundaries or
non-Rust-first applications:

```text
apps/<app-name>/
  web/
  docs/
  rs/
```

Do not create a separate language workspace for every app or service by default.
Create a separate workspace only when there is a real reason, such as independent
toolchain constraints, release isolation, dependency graph isolation, or a
different package manager ecosystem. If a separate workspace is introduced, it
should still follow the same taxonomy and dependency rules.

## Consequences

This gives the repository one durable vocabulary:

- `crates` describes code packaging and reuse.
- `services` describes production runtime ownership.
- `apps` describes product-facing application surfaces.
- `cli` describes command-line application surfaces.

The same binary can still emit OpenTelemetry under a `service.name` when useful.
Telemetry identity alone does not determine repository placement; operational
ownership does.

## References

- [OpenTelemetry glossary](https://opentelemetry.io/docs/concepts/glossary/)
- [OpenTelemetry resource attributes](https://opentelemetry.io/docs/concepts/resources/)
- [OpenTelemetry service semantic conventions](https://opentelemetry.io/docs/specs/semconv/resource/service/)
- [Kubernetes workloads](https://kubernetes.io/docs/concepts/workloads/)
- [Heroku Procfile process types](https://devcenter.heroku.com/articles/procfile/)
