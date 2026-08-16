---
number: "0028"
slug: decider-admission-control-and-backpressure
status: accepted
date: 2026-07-15
---

# ADR#0028: Admission Control for Decider Command Execution

## Context

`trogon_decider_wasm_runtime::engine::WasmDeciderEngine::new_store` creates a
fresh `wasmtime::Store<GuestState>` for every [command](../glossary/command) execution, with a fixed
per-store memory ceiling (`DEFAULT_MAX_MEMORY_BYTES`, 64 MiB) and a [fuel](../glossary/fuel)
budget consumed per guest export call. `WasmCommandExecution::execute` calls
`engine.new_store()` unconditionally on every execution, with no check on how
many stores already exist concurrently. The
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

Introduce a `CommandAdmission`-shaped hook (a bounded-concurrency permit,
semaphore-shaped) acquired at the top of `CommandExecution::execute` and
`WasmCommandExecution::execute`, released on completion (RAII permit), before
any I/O or [wasmtime](../glossary/wasmtime) work begins.

The hook is a **seam, not a policy**. The shared crates own the trait, the
`AdmissionLimit` value object, the `OverloadedError` rejection, and one
semaphore-shaped implementation callers may reuse. They do not own a bound:
no default limit is chosen anywhere in the decider stack, because the number
worth choosing is `admission_limit x max_memory_bytes` against a specific
host's memory budget, which is a deployment fact and not a property of any
decider.

Admission is therefore **opt-in and no-op by default** (Open Question 2,
resolved): the hook is a type parameter on the execution builder defaulting
to a no-op that admits unconditionally, so an execution nobody configured
carries no limiter, no counter, and no runtime branch. Mandatory-on was
rejected because it forces the crate to invent the one number it has no basis
to invent, and because it would change the runtime behavior of every existing
caller to enforce a bound none of them asked for. Adding `Overloaded` to the
shared error enums is source-breaking for exhaustive matches either way, as
noted below; what opt-in buys is that the variant is unreachable at runtime
until a caller wires a limiter.

`admit` is **synchronous**, returning `Result<Permit, OverloadedError>` rather than
a future. An admission decision that can await is a queue, and queueing at
this layer is rejected below; a sync signature makes that structural rather
than advisory. A host that wants a bounded wait before shedding owns that
wait in its own dispatch loop, before it builds the execution.

The hook sits at the execution layer, not inside
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

Rejected as the sole placement, and partly adopted (Open Question 1,
resolved). Each consumer already owns a dispatch loop (the scheduler's
worker, the gateway's handler); a semaphore there bounds the same
concurrency without the shared crates growing a scheduling concern, and each
consumer sizes its own bound. Its costs, both named in the original draft:
the bound is per-consumer rather than per-host, so two consumers in one
process do not share a budget, and nothing in the shared crates can assert
an execution was ever admitted.

The seam resolves both without importing a scheduling policy into the
decider stack. The shared crates contribute the shareable limiter *type*, so
two consumers in one process can hold clones of one budget, which a
consumer-local semaphore cannot express: there is no shared vocabulary for
the two to agree on. Sizing, ownership, and the decision to enforce anything
at all stay with the consumer, which is what the domain-level admission bar
actually asks for. What the bar rejects is business or operational *policy*
in a business-agnostic crate; a permit trait with a no-op default is not
policy, it is the same shape the crate already uses for `SnapshotPolicy`,
`SnapshotFailurePolicy`, and `SnapshotTaskScheduler`, all of which are
operational concerns injected by the caller.

## Non-Goals

- Specifying the limiter's exact algorithm (token bucket, semaphore, leaky
  bucket). Only that one exists at a defined layer with a defined error
  contract.
- Per-[tenant](../glossary/tenant) fairness or quality-of-service scheduling. A global or
  per-command-type bound is in scope; weighting by tenant is fully open --
  [ADR#0027](./0027-decider-multi-tenancy-primitive.md) deliberately leaves
  the tenant vocabulary to consumers and keeps only a storage-resolution
  scope, so there is no platform-level tenant identity for a limiter to
  weight by, and no follow-on composition is implied here.
- Bounding NATS-level publish/consume throughput. Only host-side execution
  concurrency for command dispatch is in scope.
- Changing wasmtime fuel or memory defaults themselves. The admission
  limiter is additive to those existing knobs, not a replacement for them.

## Resolved Questions

1. **Placement.** Resolved: the seam lives in the shared execution layer, the
   policy and its sizing stay with the consumer. See the consumer-boundary
   alternative above for the rebuttal.
2. **Default behavior.** Resolved: opt-in, no-op by default, as a defaulted
   type parameter on the execution builder. See the Decision above.

Both readings of question 2 were source-breaking for exhaustive matches on
`CommandError`/`WasmCommandError`, and the accepted one still is. That cost
was unavoidable and is recorded in the Consequences.

## Consequences

- Command execution paths gain a new failure mode (rejected for lack of
  admission), reachable only where a caller wired a limiter, and a breaking
  addition to `CommandError`/`WasmCommandError` that existing callers must
  handle whether or not they wire one, the same category of change draft
  [ADR#0026](./0026-command-authorization-principal.md) proposes for its
  authorization variant.
- A host that never configures a limiter is exactly as unprotected as it is
  today. This ADR ships the mechanism and the vocabulary; it does not, by
  itself, bound any running host. The L5 NATS host is the first consumer
  expected to configure one, and it is where the
  `admission_limit x max_memory_bytes` sizing becomes a concrete number.
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
- [ADR#0027: Declared Subject Scope for Decider Stream Resolution](./0027-decider-multi-tenancy-primitive.md)
