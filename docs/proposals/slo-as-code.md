# Proposal: SLO-as-code semantics

Status: draft
Tracks: WI-21 (MS_AGENT_GOV_TOOLKIT_WORKITEMS.md)
Kind: concept port, design only. No implementation in this change.

## Why

The Agent Governance Toolkit (AGT) Python implementation
(`agent-governance-python/agent-sre/src/agent_sre/slo/spec.py`) has a small,
useful idea buried inside a much larger stateful SLO engine: SLOs should be
defined as version-controlled, human-editable text, with a lightweight
inheritance mechanism so a fleet of similar agents does not have to repeat
the same SLI thresholds in every file. That idea is language-agnostic. The
Python `pydantic`/YAML implementation is not: it depends on a runtime
object graph (`SLOSpec`, `ErrorBudgetPolicy`, in-memory `resolve_inheritance`)
that this repository has already ruled out of scope (see Non-goals below and
the tracker's own non-goals section).

This document proposes what the concept looks like translated into this
repository's conventions: TOML instead of YAML (ADR 0007), typed Rust value
objects instead of `pydantic.BaseModel` (crates/AGENTS.md), and an explicit
statement of what is in scope now versus deferred.

## Source semantics being ported

`agent-governance-python/agent-sre/src/agent_sre/slo/spec.py` defines
`SLOSpec` as a flat pydantic model with an optional `inherits_from: str | None`
field, and a free function `resolve_inheritance(specs: list[SLOSpec]) ->
list[SLOSpec]`. The resolution algorithm, restated precisely so it can be
re-implemented without the source:

1. Index all specs by name in a `by_name` map. Keep a second `resolved` map,
   initially empty, used both as a memo and as cycle detection.
2. To resolve a spec:
   - If it is already in `resolved`, return the cached result (memoization).
   - If it has no `inherits_from`, it resolves to itself; cache and return it.
   - Otherwise look up the parent by name in `by_name`. If the parent name is
     unknown, fail (unknown-parent error).
   - Recursively resolve the parent first.
   - Merge: take the parent's fields as the base, then overlay every field
     the child explicitly set (child wins on scalar/collection fields other
     than the two named below).
   - `labels` and `metadata` are the two exceptions: these merge additively,
     key-by-key, with the child's keys winning on conflict, rather than the
     child's map wholesale replacing the parent's.
   - Drop the inheritance marker itself from the merged result (a resolved
     spec no longer says who it inherited from).
   - Cache the merged, fully-resolved spec under the child's name and return it.
3. Resolve every spec in the input list this way and return the resolved list.

The important, easy-to-miss property is cycle safety by construction, not by
an explicit cycle check: because a spec is written into `resolved` only after
its parent has finished resolving, and a spec being resolved is not yet in
`resolved`, a naive translation of this exact recursion into a language
without automatic recursion-depth guards (Rust does not blow up gracefully on
stack overflow) will hang or crash on a real cycle (A extends B extends A)
instead of erroring cleanly. The Python version does not actually handle
cycles either; it recurses until Python's interpreter recursion limit trips.
**This proposal explicitly does not carry that gap forward.** Section
"Cycle-safe resolution algorithm (Rust)" below specifies an explicit
in-progress marker so a cycle is a typed error, not a stack overflow.

The merge semantics and the recursive-memoized structure above mirror AGT's
`agent_sre.slo.spec.resolve_inheritance`. No AGT source is copied into this
repository; only the described algorithm is re-expressed in Rust, and
hardened for cycle detection.

## (a) SLO object model as Rust value objects

Per `crates/AGENTS.md`: prefer domain-specific value objects over primitives,
one type per file, typed errors (never `String`), validated at construction
so invalid instances are unrepresentable. The sketch below shows type
signatures only, no bodies, and omits derives that are implementation detail.

The field shapes are drawn from `AGENT-SRE-GOVERNANCE-1.0.md` section 3
(SLO Objectives) and section 4 (Service Level Indicators), reduced to the
subset that is representable as static, version-controlled config, i.e. with
no runtime measurement state.

```rust
// slo_name.rs
pub struct SloName(String); // non-empty, unique-within-file; NOT unique-within-fleet (that is a load-time check, not a construction invariant)
pub enum SloNameError { Empty }
impl SloName {
    pub fn new(value: impl Into<String>) -> Result<Self, SloNameError>;
    pub fn as_str(&self) -> &str;
}

// sli_kind.rs
pub enum SliKind {
    TaskSuccessRate,
    ToolCallAccuracy,
    ResponseLatency { percentile: Percentile },
    CostPerTask,
    PolicyCompliance,
    DelegationChainDepth,
    HallucinationRate,
    CalibrationDelta,
}
impl SliKind {
    pub fn metric_name(&self) -> &'static str; // e.g. "task_success_rate"
    pub fn is_inverted(&self) -> bool; // true for DelegationChainDepth, HallucinationRate, CalibrationDelta
}

// percentile.rs
pub struct Percentile(f64); // (0.0, 1.0]
pub enum PercentileError { OutOfRange(f64) }
impl Percentile {
    pub fn new(value: f64) -> Result<Self, PercentileError>;
}

// sli_target.rs
pub struct SliTarget(f64); // meaning depends on SliKind::is_inverted(); construction alone cannot validate that cross-field rule, see note below
pub enum SliTargetError { NotFinite(f64) }
impl SliTarget {
    pub fn new(value: f64) -> Result<Self, SliTargetError>;
    pub fn value(&self) -> f64;
}

// time_window.rs
pub enum TimeWindow { Hour1, Hour6, Day1, Day7, Day30, Custom(NonZeroU32) } // seconds for Custom
impl TimeWindow {
    pub fn as_seconds(&self) -> u32;
    pub fn label(&self) -> &'static str; // "1h", "6h", "24h", "7d", "30d"
}

// sli_spec.rs
pub struct SliSpec {
    kind: SliKind,
    target: SliTarget,
    window: TimeWindow,
}
impl SliSpec {
    pub fn new(kind: SliKind, target: SliTarget, window: TimeWindow) -> Self;
}

// error_budget_window.rs
// Static config only: the *policy* for how a budget is windowed and alerted on,
// not a running counter. See Non-goals: no ErrorBudget runtime object in v1.
pub struct ErrorBudgetWindow {
    window: TimeWindow,
    burn_rate_alert: BurnRateThreshold,   // default multiplier 2.0
    burn_rate_critical: BurnRateThreshold, // default multiplier 10.0
}

// burn_rate_threshold.rs
pub struct BurnRateThreshold(f64); // > 0.0
pub enum BurnRateThresholdError { NotPositive(f64) }
impl BurnRateThreshold {
    pub fn new(value: f64) -> Result<Self, BurnRateThresholdError>;
}

// slo_spec.rs
pub struct SloSpec {
    name: SloName,
    description: String,
    service: ServiceName,           // reuse existing service-identity value object if one exists; otherwise new file
    indicators: NonEmptyVec<SliSpec>, // AGENT-SRE-GOVERNANCE-1.0 3.1: "At least one SLI"
    error_budget: Option<ErrorBudgetWindow>, // None => derive at resolve time, see 3.6
    labels: BTreeMap<String, String>,
    metadata: BTreeMap<String, toml::Value>,
}
impl SloSpec {
    pub fn indicators(&self) -> &[SliSpec];
    pub fn effective_error_budget(&self) -> ErrorBudgetWindow; // implements AGENT-SRE-GOVERNANCE-1.0 3.6 auto-derivation: 1.0 - min(target)
}
```

Notes on the translation:

- `SLOSpec.inherits_from: str | None` in Python does not appear on `SloSpec`
  above. Inheritance is a document-level, pre-resolution concept (see part
  b), not a field of the resolved domain type. A fully resolved `SloSpec`
  has no notion of a parent, matching the Python behavior of dropping
  `inherits_from` from the merged result.
- `ComparisonOp` (Python's `lte`/`gte`/`lt`/`gt`) collapses into
  `SliKind::is_inverted()` here, following AGENT-SRE-GOVERNANCE-1.0 section
  4.5: the comparison direction is a property of which SLI type it is, not
  an independently chosen operator. This removes a whole class of
  "inverted SLI compared with `gte`" bugs that the primitive-`ComparisonOp`
  design in Python allows.
- `SliTarget` cannot enforce the inverted-vs-non-inverted meaning by itself
  since that depends on the sibling `SliKind`. Per crates/AGENTS.md
  ("validate per-type, not per-aggregate"), that cross-field rule belongs in
  `SliSpec::new`, not smuggled into `SliTarget`.
- `NonEmptyVec<SliSpec>` and `ServiceName` are assumed reusable/introducible
  value objects; if `NonEmptyVec` does not already exist in the workspace,
  it is a one-file addition, not a reason to relax `indicators` to `Vec`.

## (b) TOML representation with inheritance

Per ADR 0007, TOML is the primary human-edited config format, and `Map`
values merge by key, `List` values replace unless documented additive. This
design keeps the file syntax close to the Python YAML shape but swaps
`inherits_from` for a shorter, more TOML-idiomatic `extends` key, and treats
`[labels]` and `[metadata]` as the two documented additive-merge maps
(mirroring the Python implementation's own special-casing of those two
fields).

Parent SLO:

```toml
# slo/base-agent.toml
name = "base-agent"
description = "Baseline reliability contract for all customer-facing agents"
service = "agent-mesh"

[[indicators]]
kind = "task_success_rate"
target = 0.995
window = "30d"

[[indicators]]
kind = "response_latency"
target = 5000.0
window = "1h"
percentile = 0.95

[error_budget]
window = "30d"
burn_rate_alert = 2.0
burn_rate_critical = 10.0

[labels]
tier = "standard"

[metadata]
owner = "agent-reliability-team"
```

Child SLO, overriding one indicator and tightening the label, inheriting
everything else:

```toml
# slo/checkout-agent.toml
name = "checkout-agent"
extends = "base-agent"
service = "checkout-agent"

[[indicators]]
kind = "task_success_rate"
target = 0.999
window = "30d"

[labels]
tier = "critical"

[metadata]
pagerduty_service = "checkout-agent-primary"
```

Resolved result for `checkout-agent` (what a consumer sees after
resolution; this is the shape that never has an `extends` key):

```toml
name = "checkout-agent"
description = "Baseline reliability contract for all customer-facing agents"
service = "checkout-agent"

[[indicators]]
kind = "task_success_rate"
target = 0.999
window = "30d"

[error_budget]
window = "30d"
burn_rate_alert = 2.0
burn_rate_critical = 10.0

[labels]
tier = "critical"

[metadata]
owner = "agent-reliability-team"
pagerduty_service = "checkout-agent-primary"
```

Note `indicators` above: this design follows ADR 0007's default list rule
("lists replace lower-precedence lists unless a tool documents an additive
flag"). A child that declares any `[[indicators]]` entries replaces the
parent's `indicators` list wholesale (matching the Python behavior, where
`model_dump` merge is a shallow `{**parent_data, **child_data}` and
`indicators` is not one of the two fields special-cased for additive merge).
If per-fleet experience shows list-level replacement is too coarse (for
example, wanting to override just the latency SLI while keeping the parent's
success-rate SLI), that would need a documented additive keying scheme
(match `kind`) as a v2 change, not a silent default, per ADR 0007's rule that
additive list merges must be documented.

### Cycle-safe resolution algorithm (Rust)

Same shape as AGT's `resolve_inheritance`, hardened with an explicit
in-progress state so a cycle is a typed error instead of unbounded
recursion:

```rust
pub enum ResolutionState<'a> {
    Resolved(&'a SloSpec),
    InProgress,
}

pub enum SloResolutionError {
    UnknownParent { child: SloName, parent: SloName },
    Cycle { path: Vec<SloName> },
}

pub fn resolve_inheritance(
    documents: &[SloDocument], // SloDocument = SloSpec fields + optional `extends: Option<SloName>`, pre-resolution
) -> Result<Vec<SloSpec>, SloResolutionError>;
```

Algorithm:

1. Index input documents by name into `by_name`.
2. Maintain `resolved: HashMap<SloName, SloSpec>` and `in_progress:
   HashSet<SloName>` (the explicit addition versus the Python version).
3. To resolve a document `d`:
   - If `d.name` is in `resolved`, return the cached `SloSpec`.
   - If `d.name` is in `in_progress`, return `Err(Cycle { path })`, where
     `path` is the chain of names currently being resolved (tracked on the
     call stack or an explicit `Vec` passed through the recursion).
   - Mark `d.name` as `in_progress`.
   - If `d.extends` is `None`, the resolved value is `d`'s own fields
     converted directly to `SloSpec`; cache it, unmark `in_progress`, return.
   - Otherwise look up `d.extends` in `by_name`; if absent, return
     `Err(UnknownParent { child: d.name, parent: d.extends })`.
   - Recursively resolve the parent (propagating any `Cycle`/`UnknownParent`
     error unchanged).
   - Merge fields: child's `Some(_)`/explicitly-set scalar and struct fields
     override the parent's; `labels` and `metadata` merge as maps with child
     keys winning; `indicators`, when present on the child, replaces the
     parent's `indicators` wholesale (see list-merge note above).
   - Build the merged `SloSpec` (this step reuses the value objects from
     part a, so a malformed merge, e.g. an empty `indicators` list, fails
     construction here rather than propagating downstream).
   - Cache under `d.name` in `resolved`, unmark `in_progress`, return it.
4. Resolve every document; return the first error encountered, or the full
   resolved list.

This is a depth-first walk with an explicit color marking (white implicit
via absence, gray via `in_progress`, black via `resolved`), the standard
technique for cycle detection in a dependency graph. It bounds recursion
depth to the longest acyclic inheritance chain and turns every cycle into a
`SloResolutionError::Cycle` carrying the offending path, which is strictly
better diagnostics than AGT's Python version provides today.

## (c) Where this lives: new crate, not a `trogon-service-config` extension

Recommendation: a new crate, tentatively `trogonai-slo-spec`, not an
extension of `trogon-service-config`.

Justification against ADR 0002 (Rust Crate Boundaries):

- `trogon-service-config` is a `trogon-*` crate: its public contract
  (confique loading, env/CLI/file precedence, `NatsConfigSection`-style
  sections) is infrastructure-boundary-shaped and is explicitly listed under
  ADR 0002's `-config` suffix rule as "configuration value objects and
  loaders" for *services*. SLO documents are not service runtime
  configuration in that sense: they describe a domain object (an agent's
  reliability contract), not how a binary boots (NATS URL, credentials,
  ports). Folding `SloSpec`/`SliKind`/`resolve_inheritance` into
  `trogon-service-config` would mix a domain model into an infra-loader
  crate, which ADR 0002 calls out directly: "do not create a mini-crate...
  large packages [should not] hide unrelated responsibilities."
- ADR 0002's "When To Split" criteria fit a new crate here: the SLO domain
  model is reusable by multiple, unrelated dependents (a future Datadog
  monitor generator, a future policy/gate check, a CLI validator, CI lint),
  it has a distinct dependency set (no NATS, no confique builder wiring
  needed for the value objects and resolution algorithm themselves, only
  `toml`/`serde`), and it is a coherent, independently versionable public
  API (parse, resolve, validate) with one reason to change (the SLO object
  model), separate from `trogon-service-config`'s reason to change (how
  services discover their runtime settings).
- `trogonai-*` is the correct prefix, not `trogon-*`: the SLI catalog
  (`TaskSuccessRate`, `PolicyCompliance`, `DelegationChainDepth`, and so on)
  is meaningful specifically in the context of this repo's agent governance
  model (Agent OS policy engine, AgentMesh identity), not a
  protocol/runtime-agnostic library candidate for extraction elsewhere. Per
  ADR 0002, "when the boundary is ambiguous, default to `trogonai-*`."
- The crate would still *use* `trogon-service-config`'s `load_config`
  pattern for the mechanical "read this TOML file, deserialize, validate"
  step when a consumer wants to load a directory of SLO documents at
  startup, i.e. dependency direction is `trogonai-slo-spec` (domain) called
  from whatever CLI or service loads it, consistent with ADR 0002's
  dependency-direction rule (services/apps/CLI -> domain -> std/protocol).
  It does not need to depend on confique itself; parsing raw TOML into
  `SloDocument` and resolving inheritance is pure domain logic over
  `toml::Value`/`serde`, no env/CLI merging involved, since ADR 0007's
  precedence ladder (defaults -> file -> env -> CLI) does not obviously
  apply to a reliability contract the same way it does to a NATS URL. If a
  future need for env/CLI overrides of individual SLO fields emerges, that
  is exactly the kind of "real integration requires it" case ADR 0007
  anticipates, and can be layered on top without changing the crate
  boundary decision here.

## (d) What consumes this

Being direct about the current state: nothing in this repository consumes
an SLO spec yet, and this proposal does not add a consumer. Plausible
future consumers, in rough order of how close they are to existing code:

- `trogon-gateway`'s Datadog webhook source
  (`rsworkspace/crates/trogon-gateway/src/source/datadog/`), added in
  `f16ce7f60` (feat(gateway): add Datadog webhook source), already ingests
  Datadog monitor/webhook events into the gateway's event model. A resolved
  `SloSpec` could plausibly be the input to a future generator that emits
  Datadog monitor definitions (burn-rate alert queries built from
  `ErrorBudgetWindow`), so that the gateway's existing Datadog ingestion path
  has something structured to correlate against. That generator does not
  exist; this is a plausible direction, not a commitment.
- `trogon-telemetry` is the shared observability setup crate (ADR 0002
  names it explicitly). It is a plausible place for span/metric naming
  conventions that an SLI's `metric_name()` would need to line up with
  (e.g. via `trogon-semconv`), but it has no SLO-awareness today and none is
  proposed here.
- A CI/CLI validator that just parses and resolves `*.toml` SLO documents
  and fails on `SloResolutionError` is the smallest useful consumer and the
  natural first real implementation step after this proposal, but it is out
  of scope for this document.

No enforcement, no alert firing, no monitor creation exists today. This
document specifies the object model and file format only.

## (e) Explicit non-goals

Restating and scoping the tracker's own non-goals section for this item
specifically:

- No error-budget *runtime* object in v1. `ErrorBudgetWindow` above is
  static policy (window length, alert thresholds) read from TOML. It has no
  `consumed` counter, no bounded event deque, no `is_exhausted` check
  against live traffic. AGT's `ErrorBudget` (threading.Lock-guarded,
  in-memory, mutable) is exactly the kind of Python runtime object the
  tracker's non-goals section rules out; "idiomatic rewrites on JetStream
  primitives beat FFI bridges," and a JetStream-backed burn-rate counter is
  a separate proposal with its own design questions (durability, replay,
  consumer semantics), not an extension of this file format.
- No `SLOStatus` evaluation engine (`HEALTHY`/`WARNING`/`CRITICAL`/
  `EXHAUSTED`/`UNKNOWN`) in v1. Section 3.2-3.4 of
  AGENT-SRE-GOVERNANCE-1.0 describe a stateful evaluator that needs live
  measurements; this proposal only covers the spec that such an evaluator
  would eventually read, not the evaluator.
- No circuit breaker, cost guard, or exhaustion-action execution
  (`FREEZE_DEPLOYMENTS`, `CIRCUIT_BREAK`, `THROTTLE`). Those are explicitly
  listed in the repository's own non-goals section as Python runtime
  objects that do not port.
- No SLI *collection* machinery (`collect()`, `record()`,
  `values_in_window()`). This proposal defines what an SLI's static
  configuration looks like, not how measurements are gathered or where they
  are stored.
- No fleet-wide uniqueness enforcement of `SloName` across multiple files or
  services at construction time. `SloName` only guarantees non-emptiness at
  construction; uniqueness-within-a-resolved-set is a property the
  resolution/loading step should check and is called out above as a
  load-time check, not a type invariant.
- No additive per-`kind` merge of `indicators` across `extends` chains in
  v1 (see the list-merge note in part b). Lists replace, matching ADR 0007's
  default and AGT's own shallow-merge behavior; a smarter merge is future
  work if fleets need it, not assumed here.
