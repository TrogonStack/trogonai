---
number: "0028"
slug: decider-admission-control-and-backpressure
status: draft
date: 2026-07-15
---

# ADR#0028: Admission Control for Decider Command Execution

## Context

`trogon_decider_wasm_runtime::engine::WasmDeciderEngine::new_store` creates a
fresh `wasmtime::Store<GuestState>` for every [command](../glossary/command) execution, with a fixed
per-store memory ceiling (`DEFAULT_MAX_MEMORY_BYTES`, 64 MiB) and a [fuel](../glossary/fuel)
budget consumed per guest export call. Both
`WasmCommandExecution::execute` implementations (with and without a snapshot
store) call `engine.new_store()` unconditionally at the top of every
execution, with no check on how many stores already exist concurrently. The
per-store limiter bounds one store's worst case; it says nothing about how
many worst cases can exist at once. A burst of concurrent [WASM](../glossary/wasm)-routed
commands can pin `N x 64 MiB` of linear memory well before any individual
guest ever hits its own limiter.

`DeciderRegistry` routes command types to an `Arc<WasmDeciderModule>` and is,
by its own doc comment, "built once at startup... shared read-only across
command executions" -- it holds no state about in-flight work and has no
natural place to track or reject on load.

The native path is equally unbounded: nothing in
`trogon_decider_runtime::execution::CommandExecution::execute` gates how many
concurrent executions run against the same event store. Every command
execution, native or WASM, is accepted and run immediately, with no host-level
mechanism to shed load when concurrency spikes.

## Decision

### 1. A configurable admission limiter at the execution layer

Introduce an `AdmissionLimiter`-shaped hook (a bounded-concurrency permit,
semaphore-shaped) acquired at the top of `CommandExecution::execute` and
`WasmCommandExecution::execute`, released on completion (RAII permit), before
any I/O or [wasmtime](../glossary/wasmtime) work begins. Whether an
unconfigured execution defaults to today's unlimited behavior (an opt-in
no-op slot, the shape draft
[ADR#0026](./0026-command-authorization-principal.md) proposes for its
authorizer) or the limiter is mandatory-on with a default bound is
deliberately left open (Open Question 2); the two readings differ by
exactly one unconditional behavior change to every existing caller. It sits
at the execution layer, not inside
`DeciderRegistry` or `WasmDeciderEngine`: the registry is deliberately
stateless and shared read-only, and making every `route()` lookup also
mutate a shared counter would add contention to what is today a lock-free
lookup and conflate routing with scheduling. `WasmDeciderEngine` is a
process-global wasmtime configuration wrapper, not a scheduler, and a limiter
placed only there would say nothing about the native path, which is not
wasmtime-based at all. The execution layer is the one place both paths
already funnel every command through.

### 2. A distinguishable overloaded/retry error

Add an `Overloaded` variant to `CommandError` and `WasmCommandError`,
parallel to the existing phase-tagged variants (`Decide`, `Evolve`,
`ReadStream`, `Append`, and so on). A caller can pattern-match it apart from
a domain rejection (`Decide`) or an infrastructure failure (`Append`) and
translate it into a retry-with-backoff or a shed-load response. Rejection is
immediate, not queued: an admission limiter that queues without a bound just
relocates the same memory pressure into the queue and trades a fast,
distinguishable error for hidden added latency.

### 3. Interaction with the WASM per-session memory ceiling

The admission limit and `WasmEngineConfig::max_memory_bytes` answer two
different questions and are sized together, not against each other: the
memory ceiling bounds one store's worst case, the admission limit bounds how
many worst cases can exist at once. `admission_limit x max_memory_bytes` is
the real worst-case host memory attributable to WASM guest execution, and
that product is what capacity planning budgets against, not either number in
isolation.

## Alternatives Considered

### Rely solely on the per-store memory limiter and fuel budget

Rejected. They bound one store's damage, not aggregate concurrent damage. A
burst of legitimate, individually well-behaved commands can still exhaust
host memory purely by count, with every store staying under its own limit.

### Bound concurrency inside `DeciderRegistry`

Rejected. The registry is explicitly documented as built once and shared
read-only; making it track admission state adds write contention to a lookup
path and conflates command routing with scheduling, two concerns that should
stay independently testable.

### Bound concurrency only inside `WasmDeciderEngine`

Rejected as the sole location. The engine is process-global and per-module; a
limiter placed only there cannot distinguish "no room for any WASM command"
from "no room for this specific command type," and it does nothing for the
native `CommandExecution` path, which is not wasmtime-based and needs the
same protection. An engine-scoped limiter remains a reasonable additional
layer specifically for guest store count, but not a replacement for a limiter
at the execution layer that covers both paths uniformly.

### Unbounded internal queueing instead of rejecting

Rejected as the default. Queueing without a bound hides the same failure
mode behind added latency instead of surfacing it as a fast, distinguishable
error; it also gives callers (schedulers, gateway dispatch) no signal to
shed or retry, unlike this codebase's existing fail-loudly posture
([ADR#0017](./0017-aauth-agent-authentication.md),
[ADR#0023](./0023-secret-management-and-key-custody-direction.md)).

### Bound concurrency at the consumer or gateway boundary, outside the decider crates

Unresolved; the alternatives above all argue placement *within* the decider
stack (registry vs. engine vs. execution layer) and never weigh leaving the
stack alone. Each consumer already owns a dispatch loop (the scheduler's
worker, the gateway's handler); a semaphore there bounds the same
concurrency without the shared crates growing a scheduling concern or a new
error variant, and each consumer sizes its own bound. Its cost: the bound
is per-consumer rather than per-host (two consumers in one process do not
share a budget), and nothing in the shared crates can assert an execution
was ever admitted. Since admission control is operational mechanism rather
than domain logic, the decider crates' domain-level admission bar cuts
against the in-crate placement unless this alternative is explicitly
rebutted (Open Question 1).

## Non-Goals

- Specifying the limiter's exact algorithm (token bucket, semaphore, leaky
  bucket). Only that one exists at a defined layer with a defined error
  contract.
- Per-[tenant](../glossary/tenant) fairness or quality-of-service scheduling. A global or
  per-command-type bound is in scope; weighting by tenant is fully open --
  [ADR#0027](./0027-decider-multi-tenancy-primitive.md)'s tenant value
  object is itself an unaccepted draft, so there is no settled primitive to
  compose with, and no follow-on composition is implied here.
- Bounding NATS-level publish/consume throughput. Only host-side execution
  concurrency for command dispatch is in scope.
- Changing wasmtime fuel or memory defaults themselves. The admission
  limiter is additive to those existing knobs, not a replacement for them.

## Open Questions

This ADR is a draft: the Decision sections above are proposals, and
`AdmissionLimiter`/`Overloaded` do not exist. No other document may treat
them as existing or scheduled surface until acceptance.

1. **Placement.** Admission control is operational mechanism, not domain
   logic. Whether it belongs inside the shared execution layer at all, or
   at each consumer's dispatch boundary (the alternative above), must be
   answered against the decider crates' business-agnostic, domain-level
   admission bar before acceptance.
2. **Default behavior.** Opt-in no-op slot versus mandatory-on with a
   default bound. The first leaves every existing caller's runtime behavior
   unchanged; the second actually protects hosts whose operators never tuned
   the knob. The choice does not move source compatibility either way: adding
   `Overloaded` to the shared error enums is immediately source-breaking for
   exhaustive matches under both readings. What the choice decides is only
   whether the runtime path that can produce the variant is live on every
   execution or only where a caller opted in.

## Consequences

- Command execution paths gain a new failure mode (rejected for lack of
  admission) -- on every path if the limiter is mandatory-on, only on
  opted-in paths otherwise (Open Question 2) -- and a breaking addition to
  `CommandError`/`WasmCommandError` that existing callers must handle, the
  same category of change draft
  [ADR#0026](./0026-command-authorization-principal.md) proposes for its
  authorization variant.
- Legitimate bursts are slowed or explicitly rejected rather than silently
  degrading the whole host, but only if callers actually implement retry or
  backoff against the new error; without that, callers just see more
  failures with no functional improvement.
- Capacity planning becomes an explicit exercise
  (`admission_limit x max_memory_bytes` for WASM); a default that is not
  tuned against real traffic can be too conservative (throttling legitimate
  load) or too permissive (not actually protecting the host).
- Gets harder: test suites that fire many concurrent commands in a tight
  loop, including this crate's own WASM execution tests, may need to account
  for the limiter rather than assume unbounded concurrency.

## References

- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0026: Command Authorization Principal and Authorizer Hook for Decider Execution](./0026-command-authorization-principal.md)
- [ADR#0027: Tenant Value Object for Decider Stream and Snapshot Resolution](./0027-decider-multi-tenancy-primitive.md)
