# WASM Decider — Working Notes

Goal: deciders compile to WASM components (WIT-defined interface) so they can be
deployed/distributed independently of the host runtime.

Status: **discovery / design discussion** — no decisions locked yet, no code started.

## Goals (why WASM) — from Q&A 2026-06-10

1. **Enforced purity / trust.** Today nothing stops a decider from doing side effects
   (wall-clock time, ID generation, …) — purity is convention, not guarantee. A WASM
   component whose world imports *nothing* makes purity structural: clock/random/network
   are denied by capability, not by code review.
2. **Universal sandbox.** The same compiled decider should run in any sandbox, including
   browsers (jco-transpiled component) — server and client can execute identical logic.
3. **Serverless DX.** Authors write decide/evolve and push the module; the platform owns
   event-store connections, subscriptions, OCC, snapshots, versioning — "FaaS for
   event-sourced business logic". Authors never touch infrastructure.

Purity refinement (consequence of goal 1): `evolve` is replayed → must be strictly
deterministic `(state, event) -> state`, no context ever. `decide` runs once per command
and is never replayed — only its output events are recorded → host *may* pass declared
inputs (now, pre-generated IDs) without breaking replay. See open decision 6.

## Current state (codebase facts)

- `trogon-decider` — pure core. `Decider` trait (`rsworkspace/crates/trogon-decider/src/lib.rs:115`):
  - `initial_state()`, `evolve(state, &event)`, `decide(&state, &command) -> Decision`
  - Trait is implemented **on the command type** itself; associated types for `State`, `Event`, errors.
  - `const WRITE_PRECONDITION: Option<WritePrecondition>` (Any / StreamExists / NoStream).
  - `Decision` = `Events(...)` (non-empty batch) **or** `Act(...)` (closure chain — each step
    sees state evolved by prior steps). Closures cannot cross a WASM boundary.
- `trogon-decider-runtime` — storage-neutral orchestration. `CommandExecution`:
  load snapshot → read stream → replay via `evolve` → `decide` → encode → append (OCC).
  Codec traits already model a bytes boundary: `EventType` (stable string) +
  `EventEncode/Decode` (`Vec<u8>`), `EventDecodeOutcome::Skipped` for foreign events.
- `trogon-decider-nats` — JetStream adapter (reads, appends w/ expected-last-sequence, snapshots).
- `trogon-scheduler` — only production consumer (4 commands, protobuf event payloads).
- ADR 0009: protobuf is the standard for durable payloads → WIT layer should frame
  **opaque byte envelopes** `(event-type: string, payload: list<u8>)`, not duplicate domain schemas.
- No WASM/WIT/wasmtime code in the repo today. Clean slate.

## Open design decisions

### 1. Boundary granularity — DECIDED 2026-06-10 ((c) + batching; delegated per A3, user veto stands)

| Option | Shape | Trade-off |
|---|---|---|
| (a) Pure mirror | export `initial-state` / `evolve` / `decide` | Host owns replay, but state round-trips the author's codec per replayed event — codec burden in every language + scaling hazard as state grows |
| (b) Coarse | one `decide(command, snapshot?, events) -> events` | Single crossing; whole event suffix materialized in one call; replay logic re-implemented per module/SDK |
| (c) Resource | guest holds state in a `session` resource; host drives the loop | No per-event state serialization; host keeps snapshot policy; "a class" — natural shape to hand-implement in any language |

Recommendation: **(c) + batched evolve** — `evolve(events: list<any-envelope>)` called
once per storage read batch (matches how `trogon-decider-nats` already reads). Crossing
overhead amortizes; the dominant remaining cost is the author's own payload decode
(identical in every option). Sketch:

```wit
package trogon:decider@0.1.0;

interface handler {
  // general opaque typed-bytes envelope for events & most payloads (decision 15).
  // host routes on `type`, never reads `payload` (decision 5). Any-SHAPED but WIT-native.
  record any-envelope { type: string, payload: list<u8> }
  // command input keeps its own type (its role/entry point; room for command-specific shape)
  record command-envelope { type: string, payload: list<u8> }
  record domain-error { code: string, message: string }

  stream-id: func(command: command-envelope) -> string;

  resource session {
    // none → guest starts from its own initial state (the trait's initial_state()
    // moved inside the sandbox; host never sees a starting value)
    constructor(snapshot: option<list<u8>>);
    evolve: func(events: list<any-envelope>) -> result<_, domain-error>;
    decide: func(command: command-envelope) -> result<list<any-envelope>, domain-error>;
    snapshot: func() -> option<list<u8>>;  // none = module opts out; platform replays from zero
  }
}

world decider {
  export handler;  // imports: none — purity by construction
}
```

Packaging: see decision 14 (granularity is the author's choice — a module may host one
standalone decider or several that share state). The invariant is the consistency
boundary, not "a stream type". Descriptor export: settled — see decision 10.

### 2. `Decision::Act` support — DECIDED 2026-06-10: v1 is events-only

- Verified: `trogon-scheduler` never uses `Act` (grep 2026-06-10 — only false positives
  like `ReconcileAction`/`AlreadyActive`).
- `Act` is a closure chain → cannot cross the boundary as-is. If ever needed in WASM,
  extend `decide`'s return with a variant for host-driven iteration — additive, doesn't
  break events-only modules.

### 3. Deployment target — NARROWED (per goals)

- Primary: first-party serverless-like **decider platform** — host service embedding
  wasmtime (NATS command subjects in, JetStream persistence, components from a registry).
  Authors push a module; platform owns all infrastructure (goal 3).
- Secondary runtime: **browser** via jco-transpiled components (goal 2) — keep the WIT
  world minimal/pure so transpilation stays trivial.
- wasmCloud / Spin: reference material only; we own the platform DX.

### 4. Polyglot ambition — DECIDED 2026-06-10

- **Core goal** (A3): authors write deciders in whatever language they prefer.
- Consequence: the WIT world must be trivially hand-implementable without an SDK
  ("a class"); per-language SDKs are thin sugar (Rust first — adapt existing `impl Decider`).
- The platform wraps all guardrails around the module; authors never touch infrastructure.

### 5. Codec ownership — DECIDED 2026-06-10: guest-owned

- The module decodes/encodes its own payloads; host sees opaque envelopes
  (type string + bytes) only. Mirrors existing `EventEncode/Decode` design; keeps WIT generic.
- First-party modules use protobuf per ADR 0009. The platform cannot (and does not)
  enforce payload format — by construction it never inspects payloads.

### 6. Time / ID inputs to `decide` — DECIDED 2026-06-10

- **Caller responsibility.** Any nondeterminism (time, fresh IDs) enters as ordinary
  command data, stamped by the application/caller before the command reaches the decider.
- No context record in the WIT `decide` signature; the decider world imports nothing and
  sees nothing but data. Browser/server contracts stay identical.
- Escape hatch: component composition — wrap the pure decider component in an outer
  component that owns nondeterministic enrichment; the inner decider stays pure.
- Note: "caller" may also be the platform's command ingress (e.g., stamp received-at into
  the command as data on dequeue if queue staleness matters) — still outside the decider.

### 7. Where replay/guardrails live — DECIDED 2026-06-10: host (delegated per A3, user veto stands)

- Replay interleaves with storage I/O (stream reads). A wrapper *component* could only own
  replay by importing I/O capabilities — reintroduces capabilities (breaks goal 1's
  "imports nothing") and forces every runtime, incl. the browser, to implement them.
- Recommendation: replay loop, loading, OCC, snapshot policy live in **host runtimes** —
  platform service (Rust, evolving `CommandExecution`) + thin browser host lib (JS).
  Written twice total; authors never see it.
- Wrapper components remain the tool for **pure middleware only**: command enrichment,
  validation — composed around the inner decider. (Event upcasting is BANNED — see
  decision 12; it is never a middleware concern.)

### 8. Trust model & publishing — DECIDED 2026-06-10 (direction)

- **Full spectrum** (A5): everyone writes modules eventually, incl. untrusted environments.
  → Platform is untrusted-ready from day one: fuel/epoch limits, memory caps, per-execution
  timeouts, instance isolation. Mechanism is uniform; trust tiers are policy on top.
- First-party publishing path: PR → merge → CI builds the module artifact → registry →
  platform pulls by digest (signing/provenance later, digest pinning from the start).
- DX idea (unscheduled): purity means given/when/then conformance tests run anywhere —
  CI on the PR, platform at upload (conformance gate), browser playground.

### 9. Browser & AI execution contexts — DECIDED 2026-06-10: test & simulation scope

- Browser use (A6): **running unit tests** of modules. AI sandboxes: the same — agents
  run/test modules safely. Possible extension: AI loads modules directly, plus an
  in-memory store, to simulate the whole system (commands in → events out, zero risk).
- Explicitly NOT in scope: optimistic UI, local-first sync — no event-source-in-browser
  investment needed.
- Consequence: the non-platform host is a lightweight in-memory **sim host**:
  `given(events) → when(command) → then(events | error)` + snapshot round-trip checks.
  Cross-language re-embodiment of `trogon-decider`'s existing `TestCase` harness
  (`trogon-decider/src/testing.rs`). Same artifact serves CI (PR gate), browser (jco),
  and AI sandboxes.
- Idea (unscheduled): expose the sim host as an MCP tool via existing mcp-nats infra so
  agents can load/test/simulate deciders over the wire.

### 10. Descriptor export & write preconditions — DECIDED 2026-06-10 (user: "we need it for sure")

Today's precedence chain (`trogon-decider-runtime/src/execution.rs:562`):
1. decider's `const WRITE_PRECONDITION` → 2. caller override (`with_write_precondition`)
→ 3. default OCC from observed position (`At(position)`, or `NoStream` if stream empty).

WASM mirrors layer 1 only — the guest's layer; 2 and 3 remain host-side unchanged:

```wit
variant write-precondition { any, stream-exists, no-stream }

record command-spec {
  command-type: string,
  // none → host default: OCC at observed position (layer 3)
  write-precondition: option<write-precondition>,
}

record descriptor {
  name: string,                  // stable module identity, e.g. "scheduler.schedules"
  version: string,               // module version (not event schema versions)
  commands: list<command-spec>,
}

descriptor: func() -> descriptor;
```

- Static metadata: host calls `descriptor()` once at module load (or registry-push) and
  caches — zero cost on the command path; mirrors the Rust `const`.
- Guest declares, host enforces (JetStream expected-sequence): a module cannot bypass
  concurrency control; worst case it weakens the guard for its own stream type only.
- Bonus: `commands` list = routable inventory per module (subscriptions, registry UI,
  M1 conformance check that a YAML suite covers every declared command).

### 11. Shared event codecs across modules — DIRECTION 2026-06-10

Scenario (user): the command-side decider and future event handlers must share the event
codec; other wasm modules should be able to bundle it.

Two sharing levels, both used:
1. **Same language**: share the generated library (`trogonai-proto`) — codec statically
   compiled into each module; proto is the contract (ADR 0009). Exists today.
2. **Cross-language / bundling**: publish a **codec library component** per stream type
   (e.g., `trogon:schedules-codec`) exporting `decode(any-envelope) -> typed-event` /
   `encode(typed-event) -> any-envelope` (decision 15), with WIT
   types **generated from proto** (no
   hand-written schema duplication). Consumer worlds import the interface; CI composes
   (`wac plug`) the codec component in — purity survives (composed pure components are
   pure; conformance gate stays "zero unresolved imports in final artifact").

DEFERRED PAST v1 (per decisions 16, 17): the TYPED-variant flavor needs a proto→WIT
emitter. That emitter is NOT exotic — it's another target in the existing multi-language
codegen pipeline (proto already feeds Rust via buffa + Elixir via elixirpb); see decision
17. v1 cross-language consumers decode bytes with their own native proto library instead.
Below describes the post-v1 typed form.

Properties:
- Typed events across languages (a Python handler gets typed variants, never touches proto).
- Same-digest composition ⇒ decider and handler share the identical decode implementation.
- **Version-faithful decode** (per decision 12): every stored version is its own variant
  case (`created-v1`, `created-v2`, …); decode NEVER converts between versions. Adding a
  version extends the variant → consumers' exhaustive matches break at compile time until
  each one explicitly handles the new case.
- Can also carry the `encode-json`/`decode-json` testing interface → YAML suites work for
  event handlers too.

Defaults: deciders use level 1 (static link; platform can't tell and doesn't care); each
stream type publishes its codec component to the registry for everyone else. The
event-handler world itself = future milestone (out of v1 scope).

#### Marshaller-as-embeddable-component (refinement 2026-06-17)

Reframe: the codec component IS the deployable embodiment of the event contract — proto
stays source of truth, but the *distributed artifact* is one `schedules-codec.wasm`
(digest-addressed, versioned, embeddable in any language) instead of `.proto` + per-team
codegen.

Layering (resolves any tension with decisions 1 & 5): codec composition lives ONE LEVEL
BELOW the host's view. Host ↔ decider stays opaque bytes; decider ↔ codec is typed events
over the canonical ABI, but internal to the composed artifact. `session.evolve(envelopes)`
internally calls imported `codec.decode(envelope) -> typed-event` then folds. After `wac`
composition the codec import is satisfied → final artifact still has zero unresolved
imports → passes purity gate. Host sees one sealed module; never knows the codec exists.

```
HOST --bytes--> [ decider logic ] --typed event (canonical ABI, internal)--> [ codec ]
        ^ decisions 1/5: opaque bytes        ^ composed via `wac`, hidden from host
```

Two embed modes trade reuse vs. copy:
- **Static link** (proto-codegen crate, one linear memory, pre-componentization):
  ZERO copy, same-language only. = level 1. Default for Rust deciders.
- **Component composition** (`wac`, separate linear memories): ONE structural copy per
  event across the canonical ABI, ANY language, shared decode by digest. = level 2.
  Honest cost: a marshaller handing typed values across the boundary itself marshals at
  the ABI (proto-decode + one copy). Pick it for cross-language + bit-identical-decode,
  NOT for speed.

Wrinkles:
- Still version-faithful (decision 12): embedding does not reintroduce upcasting.
- The exported typed variant is a PINNED WIT contract: adding `created-v2` changes the
  interface → every embedder recompiles, exhaustive matches break until handled
  (decision 12 discipline now enforced by the interface). Re-composition mandatory on
  schema growth.
- Foreign/unknown events need an explicit `unknown` outcome in `decode` (mirrors today's
  `EventDecodeOutcome::Skipped`); the CONSUMER decides skip-vs-fail — no hidden behavior.

Granularity: per-stream-type (matches packaging), NOT per-event-type.

### 12. Upcasting — DECIDED 2026-06-10: BANNED, everywhere

- User: "no upcasting ever! they upcast themselves explicitly."
- No transformation layer between stored bytes and the fold — not in the host, not in a
  wrapper component, not in the codec component. `state = fold(initial, stored events)`,
  nothing in between. Rationale: an upcaster is a hidden function whose change silently
  alters what historical streams replay to; banning it makes replay behavior change only
  via explicit, reviewed module code.
- Consumers handle every live version explicitly in their own evolve/handle (codec's
  version-faithful variant makes this compiler-enforced where the language allows).
- Accepted cost: every consumer carries explicit arms for all versions still live in
  streams; visible in code review rather than hidden in a migration layer.

### 13. No-op handling of unsupported events — DECIDED 2026-06-17

Question: if I don't need to support certain events, can I no-op them?

Yes, but only **explicit per-event no-ops**, never a catch-all:
- `Paused(_) => state` (a reviewed decision) is fine.
- `_ => state` is BANNED — it silently swallows every future event, re-creating the hidden
  behavior decision 12 forbids. Lint: deny wildcard arms on event variants.

Correctness depends on role:
- **Read side (handlers/projections): no-op freely, explicitly.** Ignoring events outside
  your concern is normal (the existing `EventDecodeOutcome::Skipped` path).
- **Write side (decider): MUST NOT no-op events that feed the state `decide` reads.**
  Dropping a real state transition → decisions made on false state. Concrete failure:
  decider handles only `CreatedV1`, someone ships `CreatedV2` (also a creation), replay
  no-ops it → state seen as MISSING → duplicate create allowed. A write-side no-op is
  lying about history, then deciding on the lie.

Unknown events (not in the codec variant — foreign or future-unknown; the explicit
`unknown` outcome from decision 11): safe default differs by role —
- read-side handler → **skip**;
- write-side decider → **fail closed / fence** (unknown event in its own stream ⇒ state
  provably incomplete ⇒ refuse to decide; rejected command beats corrupt write).

### 19. Compile-time handling: standalone vs bundle — DECIDED 2026-06-17

Q: how to handle, at compile time, a decider that is ONE command with its own state?

- **Standalone (N=1) is the trivial base case**: session state type = `<C as Decider>::State`
  read straight off the single impl; session methods delegate to C's `initial_state`/`evolve`/
  `decide`. NO cross-command reconciliation — there's nothing else to agree with.
- **The real compile-time work is the bundle (N>1)**: the session holds ONE state, so all
  bundled commands must share `State` + `Event`. Today that's convention (all delegate to
  `super::state::`), not type-checked. The macro makes it enforced: take command[0] as
  canonical `S`/`E`, emit `where Ci: Decider<State = S, Event = E>` bounds for the rest →
  clear compile error if any command drifts. (Assertions are simply empty when N=1.)
- **Optional cleaner shape (refactor the WASM work motivates, not requires)**: split the
  fused `Decider` into `StateModel` (State/Event/initial_state/evolve — scheduler's `state`
  module already IS this) + `Command` (decide, `type Model: StateModel`). Then agreement is
  a TYPE FACT (`Command<Model = M>`), standalone = "a model with one command", and it
  matches decider theory (decide/evolve/initial = the decider; commands are inputs).
- Unifies decision 14 at three scopes: standalone = self-contained base case; bundle =
  one StateModel / macro assertions; split-across-modules = shared composed fold component.
- v1/spike: keep fused `Decider` + macro assertions (no refactor). StateModel split = later.

### 18. Event identity & metadata are host-assigned — DECIDED 2026-06-17

The module is pure (goal 1) → it CANNOT mint a UUIDv7 event id or a timestamp (that's the
clock/random it's denied) and does not own trace headers. The host stamps event id +
recorded-at + headers AFTER `decide` returns (as the runtime's `event_id_generator` +
`Headers` already do). The module emits only `(type, payload)`; it never sees ids/headers.
Consistent with docs/architecture/event-metadata.md (metadata policy is the calling
boundary's, not the decider's).

### 17. Can the types come from buffa? — DECIDED 2026-06-17

Facts: `buffa` is an EXTERNAL crates.io crate (0.7.0) — not ours to extend with a WIT
backend. Its codegen emits checked-in `.rs` into `trogonai-proto/src/gen/`. The schema also
carries `elixirpb` annotations → **types are already generated for multiple languages from
one proto source** (Rust via buffa, Elixir via elixirpb).

Answer: not from buffa-the-crate, but from the **same proto source** buffa consumes — WIT
is the Nth codegen target in the existing pipeline, emitted as checked-in `gen/*.wit`
beside `gen/*.rs`. Three tiers:
- **Type strings + FileDescriptorSet**: free TODAY (buffa full-names + descriptors) — use
  for routing + test-runner JSON in v1.
- **Typed WIT domain types** (the typed codec, post-v1): via a NEW proto→WIT emitter in the
  pipeline. Must handle impedance mismatches — esp. proto OPEN enums → WIT CLOSED variant
  needs explicit `unknown` case (matches decisions 11/13); also field presence (optional→
  option), maps→list<tuple>, well-known types, recursion. A generator to build, but slots
  into an existing multi-target pipeline — not exotic.
- **Protocol WIT types** (`any-envelope`, `command-envelope`, `session`, `descriptor`):
  NOT proto-derived — hand-written stable contract (~30 lines, once).

Constraints unchanged: host↔module boundary stays opaque bytes regardless (generic host);
generated typed WIT lives only at the internal module↔codec seam. v1 ergonomic win: the
guest macro derives type strings + command dispatch from buffa reflection, so
`export_decider!` only lists WHICH structs are commands (no hand-typed type-URLs).

### 16. Guest SDK bridging model (WIT ↔ protobuf) — DECIDED 2026-06-17

User doubt: how do WIT and protobuf coexist at the programming-language level (two
codegens, two type systems)?

Answer — they never meet as TYPES, only as BYTES:
- wit-bindgen owns the WIT types (`any-envelope`, `command-envelope`, `domain-error`,
  `session`); `buffa` owns the domain types (`state_v1::State`, `v1::ScheduleEvent`, the
  command structs). The seam is `payload: list<u8>` ⇔ `Vec<u8>`. No tool needs to
  understand both type systems. THIS is what the opaque-bytes boundary (decisions 5/15)
  buys — it dissolves the friction.

The bridge already exists in the repo — the guest SDK macro reuses it, doesn't invent:
- `EventType::event_type() -> &'static str` (→ envelope `type`)
- `EventEncode::encode() -> Vec<u8>` (domain → payload bytes)
- `EventDecode::decode(EventData) -> EventDecodeOutcome` (bytes → domain; `Skipped` for
  foreign = decision 13's unknown handling)
- `buffa::Message` (v0.7.0, rust-protobuf-style: MessageField/EnumValue/DecodeError) for
  state snapshot encode/decode.

Macro-generated guest: a `Session` struct holding the proto `State`, implementing the
wit-bindgen `GuestSession` trait; its `new/evolve/decide/snapshot` call the traits above +
the author's unchanged `impl Decider`. Author writes only `impl Decider` + the
`export_decider!` command-type→struct map.

Build: one crate, cargo-component (wit-bindgen) + buffa codegen side by side, both just
Rust modules → one component. (SPIKE to confirm — see open items.)

Polyglot: identical pattern per language — WIT bindings give bytes+type; the language's
native proto lib decodes the payload (TS: ts-proto; Go: google.golang.org/protobuf; etc.).
No proto↔WIT bridge needed.

### 15. Encoding — DECIDED 2026-06-17 (scope-corrected: Any is for the TEST YAML)

`google.protobuf.Any` is the encoding for the unit-test YAML files — NOT a claim that the
whole WIT boundary is literally the proto well-known type (corrects the first pass).

- **Test YAML**: payloads are `google.protobuf.Any` canonical JSON (`@type` + proto3-JSON
  fields). Runner does proto3-JSON ⇄ binary via a `FileDescriptorSet` (standard protobuf,
  e.g. `prost-reflect`); descriptor set from the first-party proto build
  (`protoc --descriptor_set_out`), shipped beside the `.wasm`. No custom `encode-json` guest
  export. (ADR 0009: proto wire; proto3 JSON = human boundary.)
- **WIT boundary**: a general `any-envelope { type, payload }` carries events and most
  payloads; the command input keeps its own `command-envelope` type ("a general any-envelope
  for most things, distinct from command-envelope" — user). Both are Any-SHAPED (opaque typed
  bytes) WIT-native records, not literally the proto type. Host routes on `type`, never reads
  `payload` (decision 5 intact).

### 14. Packaging granularity — DECIDED 2026-06-17 (corrects earlier "one module = one stream type")

User: "each decider can be its own thing, sometimes." The `Decider` trait is per-command,
so the atom is the single decider; bundling is optional.

Invariant: a module hosts ONE consistency boundary = one `(State, Event, stream)` = one
`session`. Number of commands inside is `1..N`, author's choice. WIT contract unchanged
(descriptor `commands` is 1..N; `stream-id` per command; session holds that state).

Three valid shapes:
- **Standalone** (N=1): decider with its own state/stream → its own module. Independent
  deploy/version/limits/blast-radius — serves goals 1 & 3; often the better grain.
- **Bundle** (N>1, shared state): e.g., scheduler's 4 commands share `state_v1::State` +
  one `evolve` → one `schedules.wasm`. Simplest when deploying together is fine.
- **Split-but-shared-state**: separate modules that share a state → ALLOWED, but the fold
  (`evolve`) + state type + codec MUST be ONE shared composed component (same mechanism as
  decision 11), never duplicated. HARD RULE: shared state ⇒ one authoritative `evolve`;
  divergent copies = divergent replay = corruption (decision 12 spirit). Standalone
  deciders then carry only their own `decide` + `WRITE_PRECONDITION`, composed against the
  shared fold/codec.

v1 scope: one session (one consistency boundary) per module. Multiple independent
session-kinds in one module = possible future extension, not v1.

## Milestone 1 — YAML test runner (decided 2026-06-10, A7)

Definition of done: `decider-test <module.wasm> <tests.yaml>` — load a compiled decider
component, execute given/when/then scenarios from YAML, report pass/fail. No NATS, no
infrastructure.

Pieces:
1. WIT package `trogon:decider` — handler interface (session resource) + optional
   `testing` interface for human-readable payloads.
2. Rust guest SDK — `trogon-scheduler` commands compile to `schedules.wasm`, decider
   code unchanged.
3. Sim host library (wasmtime, in-memory) — later reused by platform/browser/AI hosts.
4. CLI test runner consuming YAML suites.
5. Benchmark (criterion): instantiate + replay + decide at 1/10/100/10k events —
   validates the performance expectations below with real payloads.

YAML payload encoding: RESOLVED — see decision 15. Test YAML uses `google.protobuf.Any`
canonical JSON (`@type` + proto3-JSON fields); runner does proto3-JSON ⇄ binary via a
`FileDescriptorSet`. No custom `encode-json` interface.

Sketch of a suite (mirrors existing `TestCase` tests 1:1):

```yaml
# payloads are google.protobuf.Any canonical JSON: @type + proto3-JSON fields (decision 15)
suite: schedules
scenarios:
  - name: creating a new schedule succeeds
    given: []
    when:
      "@type": type.trogon.ai/scheduler.schedules.v1.CreateSchedule
      id: "sched-1"
      schedule: { "...": "..." }
    then:
      events:
        - "@type": type.trogon.ai/scheduler.schedules.v1.ScheduleEvent
          schedule_created: { schedule_id: "sched-1" }

  - name: creating an existing schedule fails
    given:
      - "@type": type.trogon.ai/scheduler.schedules.v1.ScheduleEvent
        schedule_created: { schedule_id: "sched-1" }
    when:
      "@type": type.trogon.ai/scheduler.schedules.v1.CreateSchedule
      id: "sched-1"
    then:
      error: { code: already-exists }   # domain-error is a WIT result, not an Any payload
```

## Performance expectations (order-of-magnitude; validate in M1 benchmark)

| Cost | Order | When paid |
|---|---|---|
| Compile module (wasm→native) | ms–100s ms | once at deploy — precompile at registry-push; never on command path |
| Fresh instance + session | ~10–50µs | per command (wasmtime pooling allocator) |
| One host↔guest call | ~0.1–1µs | ~4–6 calls per command (batched evolve) |
| Envelope byte copy | ~0.1µs/KB | per envelope |
| Guest decode + logic | ~1–10µs | per command (1–10 events) — paid natively too |
| **Sandbox total (1–10 events)** | **≪ 100µs** | |
| JetStream read + OCC append | ~1–5ms | per command — dominates; identical for native deciders today |

**M1 benchmark (2026-06-21, Light decider, native sim host):**

| Scenario | p50 (≈) | p99 (≈) |
|---|---|---|
| `decide/initial` | 65µs | 65µs |
| `evolve/1` | 67µs | 69µs |
| `evolve/10` | 67µs | 67µs |
| `evolve/100` | 80µs | 81µs |
| `evolve/10000` | 1.65ms | 1.65ms |

- Sub-ms guarantees: YES for decider execution (sim host / sandbox slice); NO for
  end-to-end production commands — that budget is JetStream RTTs, unchanged by WASM.
- SLO structure: decider-execution SLO (e.g., p99 < 200µs for ≤10 events) layered under
  an end-to-end command SLO (low single-digit ms p99, governed by NATS topology).
- Budget hazards + mitigations: cold start → precompile/warm at deploy; long streams →
  snapshots (host policy); CPU metering → epoch interruption for trusted tier (≈free),
  fuel counting only where the trust tier demands it.

## Consolidated WIT v0.1.0 (illustrative; supersedes the sketch in decision 1)

```wit
package trogon:decider@0.1.0;

interface types {
  record any-envelope     { type: string, payload: list<u8> }   // opaque typed bytes (5,15)
  record command-envelope { type: string, payload: list<u8> }
  record domain-error     { code: string, message: string }

  variant write-precondition { any, stream-exists, no-stream }    // decision 10
  record command-spec { command-type: string, write-precondition: option<write-precondition> }
  record descriptor   { name: string, version: string, commands: list<command-spec> }

  variant decide-error {                                          // P1.4: rejection vs fault
    rejected(domain-error),   // business "no" — host acks, no retry
    faulted(domain-error),    // decode/unknown/internal — host fences / dead-letters
  }
}

interface handler {
  use types.{any-envelope, command-envelope, descriptor, domain-error, decide-error};

  descriptor: func() -> descriptor;                               // static, cached at load
  stream-id:  func(command: command-envelope) -> result<string, domain-error>;

  resource session {
    constructor(snapshot: option<list<u8>>);                      // none → initial_state()
    evolve:   func(events: list<any-envelope>) -> result<_, domain-error>;
    decide:   func(command: command-envelope) -> result<list<any-envelope>, decide-error>;
    snapshot: func() -> option<list<u8>>;
  }
}

world decider { export handler; }   // imports: NONE
```

## Pre-implementation gap analysis (2026-06-17)

Grounded by Cargo.lock inspection: buffa deps are wasm-clean (base64, bytes, compact_str,
ecow, hashbrown, once_cell, serde, serde_json, smol_str, thiserror) and buffa pulls NO
getrandom. getrandom in the workspace comes from host-side crates (uuid, tokio), not the
guest path. No wasm32/cargo-component config exists yet (greenfield).

**P0 — must PROVE before committing (the spike):**
1. buffa compiles to wasm32 + the guest crate's dep tree pulls no getrandom / wasi:random.
2. cargo-component + buffa checked-in codegen coexist in one crate.
3. Final artifact has ZERO imports — verify `wasm-tools component wit out.wasm`. Literal
   proof of goal 1; everything depends on it.

**P1 — finalize in the proposal (recommendations noted):**
4. Error model: split business `rejected` vs processing `faulted` (DecideError → rejected;
   EvolveError / decode / unknown → faulted). Drives host ack/retry. (Baked into consolidated WIT.)
5. Snapshot schema versioning: tag snapshot with state-schema version; mismatch → discard +
   replay from zero (safe — snapshots are a cache, decision 12 doesn't apply).
6. Test comparison: proto-message equality (decode both sides), not raw bytes (proto wire
   not canonical).
7. Residual determinism: zero-imports kills clock/random/net, but `evolve` must avoid
   map-iteration-order dependence (BTreeMap/sorted, not HashMap). Lint + guidance.
8. Crate layout per ADR 0002: finalize names (proposed: trogon-decider-wit /
   trogon-decider-guest / trogon-decider-sim / decider-test).

**P2 — deferred (not v1):** typed cross-language codec + proto→WIT emitter (decision 17);
platform service (NATS/JetStream host, fuel/epoch limits, multi-tenant isolation);
registry/OCI distribution; browser jco host; Act (decision 2).

## Decision log

- 2026-06-10 — **Goals ranked** (Q&A A1): (1) enforced purity via capability denial,
  (2) run-anywhere sandbox incl. browsers, (3) serverless DX where the platform owns all
  infrastructure. Deployment target narrowed to first-party platform + browser as second
  runtime; wasmCloud/Spin are reference only.
- 2026-06-10 — **Nondeterminism is caller responsibility** (Q&A A2): commands carry any
  time/IDs as ordinary data; `decide` gets no host-provided context record; the decider
  world imports nothing. Composition (wrapper component) is the enrichment escape hatch.
- 2026-06-10 — **Polyglot authorship confirmed as core goal** (Q&A A3): any language;
  platform provides the guardrails; WIT world must be trivially hand-implementable.
- 2026-06-10 — **Contract shape decided by delegation** (A3 "you tell me"; Q4 couldn't be
  evaluated by user yet — veto stands once demo-able): session resource + batched evolve,
  zero imports; replay/OCC/snapshots in host runtimes; wrappers = pure middleware only;
  one module per stream type, commands routed by `command-type`.
- 2026-06-10 — **v1 is events-only** (`Act` unused in scheduler; additive extension path
  reserved) and **codec is guest-owned** (host never inspects payloads; proto per ADR 0009).
- 2026-06-10 — **Trust model: full spectrum** (Q&A A5): everyone eventually, incl.
  untrusted; limits/isolation built in from day one (mechanism uniform, trust tiers are
  policy). First-party path: PR → CI → artifact → registry, pulled by digest.
- 2026-06-10 — **Browser & AI contexts = test/simulation** (Q&A A6): unit testing in the
  browser, AI sandboxes running/testing modules, possibly AI-hosted modules + in-memory
  store for safe simulation. No local-first/optimistic-UI scope. The in-memory sim host
  (same artifact for CI / browser / AI) becomes a first-class deliverable.
- 2026-06-10 — **Milestone 1 = YAML test runner** (Q&A A7): compiled module + YAML
  given/when/then suite → run → report. Tests-as-data: language-agnostic, reusable as the
  platform conformance gate (A5), AI-authorable (A6). Goals Q&A complete.
- 2026-06-10 — **Write preconditions required; descriptor export designed** (user
  confirmation): per-command `option<write-precondition>` in a static `descriptor()`
  export, mirroring the Rust const (layer 1 of execution.rs:562 precedence); host keeps
  override + observed-position OCC default. Guest declares, host/JetStream enforces.
- 2026-06-10 — **Codec sharing direction** (user scenario): proto = schema truth; codec
  library component per stream type for cross-language bundling via composition.
- 2026-06-10 — **Upcasting BANNED everywhere** (user: "no upcasting ever"): no transform
  between stored bytes and the fold; codec decode is version-faithful (variant case per
  version); consumers explicitly handle every live version themselves.
- 2026-06-17 — **Marshaller-as-embeddable-component** refinement (user): codec component =
  deployable event contract; composition is internal to host's bytes view; static-link
  (zero-copy, same-lang) vs `wac` compose (1 copy, any-lang, by-digest); still
  version-faithful; typed variant is a pinned WIT contract.
- 2026-06-17 — **No-op policy** (user Q): explicit per-event no-ops OK, catch-all `_`
  BANNED; read-side may skip freely, write-side must not drop state-affecting events and
  fails closed on unknown events in its own stream.
- 2026-06-17 — **Packaging granularity = author's choice** (user: "each decider can be its
  own thing, sometimes"), correcting earlier "one module = one stream type". Atom is the
  per-command decider; module = one consistency boundary, 1..N commands. Standalone is
  often preferable (independent deploy). Splitting shared-state deciders requires ONE
  shared composed `evolve`/state/codec — never duplicated (divergence = corruption).
- 2026-06-17 — **Compile-time standalone vs bundle** (user Q): N=1 standalone is the
  trivial base case (session state = the command's own `::State`, no reconciliation); N>1
  bundle enforces shared State/Event via macro-emitted `Decider<State=S, Event=E>` bounds.
  Optional cleaner refactor: split fused `Decider` into `StateModel` + `Command` (agreement
  becomes a type fact). v1 keeps fused trait + assertions.
- 2026-06-17 — **Gap analysis + event metadata** (user: "what are we missing"): P0 spikes
  (buffa→wasm zero-imports, cargo-component×buffa, verify zero imports), P1 design (error
  model rejected/faulted, snapshot versioning, proto-equality test compare, evolve
  determinism, crate layout), P2 deferred. Decision 18: event id/timestamp/headers are
  host-assigned (module is pure). Consolidated WIT v0.1.0 captured (illustrative).
- 2026-06-17 — **Types from buffa?** (user Q): not from the external buffa crate, but from
  the same proto source (WIT = Nth codegen target, like Elixir already is). Type
  strings/descriptors free today; typed WIT domain types via a new proto→WIT emitter
  (post-v1, must map open enums → explicit `unknown`); protocol WIT types hand-written.
  Host boundary stays bytes regardless.
- 2026-06-17 — **Guest SDK bridging model** (user doubt re: WIT↔proto at language level):
  WIT and protobuf meet only at bytes, never as types — the opaque-bytes boundary dissolves
  the friction. The bridge already exists as `EventType`/`EventEncode`/`EventDecode` +
  `buffa::Message`; the SDK macro reuses them. Typed cross-language codec (proto→WIT gen) is
  the only real bridge tool — deferred past v1. Spikes queued: buffa×cargo-component, buffa
  JSON/dynamic surface.
- 2026-06-17 — **Encoding, scope-corrected** (user: "I meant in the unit-test YAML files;
  keep a general any-envelope for most things, distinct from command-envelope"):
  `google.protobuf.Any` canonical JSON is the TEST YAML encoding (runner uses a
  FileDescriptorSet; `encode-json` guest export dropped). WIT boundary uses a general
  `any-envelope { type, payload }` for events/most payloads + a distinct `command-envelope`
  — Any-shaped WIT records, not the literal proto type. Host routes on `type` only
  (decision 5 intact). Resolves the open M1 encoding question.

## Next steps

- [x] Boundary shape + host-owned replay — decided (delegated; veto stands once demo-able)
- [x] `Act` — scheduler doesn't use it; v1 = events-only
- [x] Codec — guest-owned opaque envelopes
- [x] Q5: trust ring — full spectrum incl. untrusted; platform untrusted-ready from day one
- [x] Q6: browser/AI = test & simulation contexts; in-memory sim host becomes a
      first-class deliverable; no local-first/event-sync investment
- [x] Q7: first milestone = YAML-driven test runner against a compiled module
- [x] Goals Q&A complete (A1–A7) — see `wasm-goals.qa.md`
- [x] Execution plan drafted + adversarially critiqued → **`PLAN.md`** (root)

→ **Execution plan: see `PLAN.md`.** Phases R (prereq refactor) → M0 (spike) → M1 (A WIT
  crate, B macro+SDK, C sim host, D CLI, E scheduler port, F CI+bench), with per-task
  acceptance criteria and go/no-go gates.

### Blockers the grounding workflow surfaced (2026-06-17 — fold into implementation)

- **wasm-clean boundary is narrow.** Guest path must avoid `tokio` (via
  `trogon-decider-runtime`), `uuid`→`wasm-bindgen` (via runtime), and `chrono`→`wasm-bindgen`
  (via scheduler domain). `wasm-bindgen` targets wasm32-unknown-unknown only → hard link
  error on wasip2.
- **Prereq refactor R-1**: move codec traits (`EventType`/`EventEncode`/`EventDecode`/
  `EventData`/`EventDecodeOutcome`) from `trogon-decider-runtime` (tokio) into `trogon-decider`
  (wasm-clean); re-export back. Guest SDK depends on `trogon-decider`, never `-runtime`.
- **Commands need a proto wire type** (R-2): scheduler command structs are plain domain
  structs, NOT proto messages — no `buffa::MessageName`. Add proto command messages +
  `TryFrom` + explicit type-URL consts; `export_decider!` takes the type-URL explicitly.
- **`chrono` blocks porting the scheduler as-is** → Light decider is the M0/early-M1
  vehicle; scheduler port (M1-E) gated on resolving chrono (swap to `time`/proto timestamps).
- **HashMap (incl. proto `map<>`) imports `wasi:random`** on wasip2 → BTreeMap/sorted in guest.
- **WIT signatures (authoritative = consolidated v0.1.0)**: `stream-id -> result<string,
  domain-error>`, `evolve -> result<_, domain-error>` (NOT decide-error), `decide ->
  result<list<any-envelope>, decide-error>`; `EventType::event_type()` returns `Result`.
  The decision-1 WIT sketch is superseded — delete on next pass.
- **Tooling discipline**: install cargo-component unpinned, then pin it + the matched
  wit-bindgen version together; verify wasmtime / prost-reflect API names before coding;
  `SimHost` import check uses `wasm-tools component wit` on a temp file (matches AC-0.2).

## Implementation log (2026-06-21)

### Prerequisite R — done
- **R-1**: Codec traits live in `trogon-decider`; re-exported from `-runtime`.
- **R-2 (command wire)**: Proto command messages + `TryFrom` + stable `TYPE_URL` consts for
  Light (`TurnOn` → `TurnOnCommand`). Guest `export_decider!` takes explicit `type_url` +
  `proto = ...` per command.

### M0 gate — closed
- **Build target**: `wasm32-unknown-unknown` + `cargo component build` (zero world imports).
  `wasm32-wasip2` pulls WASI imports via cargo-component — not used for guest artifacts.
- **Pinned toolchain**: `cargo-component 0.21.1`, `wasm-tools 1.250.0`, `wit-bindgen 0.41.0`,
  `wasmtime 29.0.1`.
- **Import check API**: `trogon-decider-sim::assert_zero_imports` shells out to
  `wasm-tools component wit` (with `wasmparser` fallback for plain core modules).
- **Smoke**: `trogon-decider-sim` loads spike/light wasm via wasmtime 29; session teardown via
  `ResourceAny::resource_drop`.
- **Guest dep tree**: no uuid/chrono/tokio/wasm-bindgen on the Light decider path.

### M1 — complete (2026-06-21)

**Status: M1 done.** YAML test runner + scheduler WASM bundle shipped.

- **A–D**: WIT crate, `export_decider!` bundle + guest-sdk, sim host + `SimFixture`,
  `decider-test` CLI with typed `trogonai-proto` JSON codec (Light + schedules).
- **B-3/B-9**: four-command `trogon-schedules-decider` bundle; zero world imports verified.
- **B-6**: trybuild compile-fail tests for non-`Decider` types and bundle Event mismatch.
- **C-7**: `test-support` feature exposes `SimFixture` (light + schedules wasm paths).
- **D-3**: typed module dispatch via `decider-test` codec; generic `prost-reflect`
  descriptor pool still deferred.
- **E-1 chrono verdict (gate)**: guest path uses `chrono` with `default-features = false`
  (no `clock`/wasmbind); `schedule-validation` feature gates `chrono-tz` + `rrule` so the
  WASM guest tree stays wasm-bindgen-free. Host scheduler keeps full validation.
- **E-2..E-5**: `trogon-scheduler-domain` extracted; `trogon-schedules-decider.wasm` builds
  zero-import; `schedules.yaml` (7 scenarios) all-pass; `cargo test -p trogon-scheduler` green.
- **F**: CI builds Light + schedules wasm, runs both YAML suites; criterion benches recorded
  above (sandbox ≪100µs for ≤10 events — validated).
