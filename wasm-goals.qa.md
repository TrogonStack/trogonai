# Q&A: WASM Decider Goals

Session started: 2026-06-10
Related: `WASM_TODO.md` (decisions distilled there as they land)

---

**Q1:** What is the primary problem you want WASM to solve for deciders? Independent deploy/distribution of business logic without redeploying the host, sandboxed isolation (multi-tenant / untrusted code), polyglot authorship, or something else?

**A1:** Three motivations:
1. **Trust / enforced purity** — today nothing prevents side effects inside a decider
   (e.g., wall-clock time, ID generation), so the code can't be truly trusted to be pure.
   WASM sandboxing makes purity enforceable.
2. **Universal sandbox** — run the same decider code in any sandbox, including browsers.
3. **Serverless DX** — authors shouldn't worry about infrastructure; push the decider to a
   platform that handles the event-store connection and "the whole shebang", like serverless/FaaS.

---

**Q2:** When a decider legitimately needs wall-clock time or fresh IDs to decide (e.g., "is this schedule's start in the past?"), how should it get them — (a) host passes them as explicit declared inputs to `decide` (a context record: now, pre-generated IDs), or (b) forbidden entirely — commands must carry any time/IDs as ordinary data fields stamped by the caller/edge?

**A2:** Option (b) — any legitimate nondeterminism or side effect is the **caller's
responsibility**: the application stamps it into the command as ordinary data before it
reaches the decider. If enrichment must live close to the decider, wrap the pure decider
component inside another WASM component that owns that part. The decider module itself
stays pure (imports nothing).

---

**Q3:** Boundary granularity for the WIT world — (a) pure mirror (`initial-state` / `evolve` / `decide`, state serialized across the boundary per replayed event), (b) coarse single call (`decide(command, snapshot?, events) -> events`, guest replays internally), or (c) a WIT `resource` holding state in guest memory (host drives the replay loop, state never crosses except for snapshots)?

**A3:** Unsure whether guest or host should own replay — it's a performance + DX trade-off;
asked for a recommendation. New constraint surfaced: **polyglot authorship is a core
motivation** — authors write deciders in whatever language they prefer, and the platform
wraps the guardrails (loading, replaying, etc.) around them, either in the host or via a
wrapper WebAssembly component.

---

**Q4:** Confirm the recommended shape? (1) Author-facing WIT world = zero imports, exporting a `session` resource: `constructor(snapshot?)`, batched `evolve(list<event-envelope>)`, `decide(command) -> events`, optional `snapshot()`. (2) Replay loop + guardrails live in the host runtimes (platform service in Rust, thin JS host lib for browser), NOT in a wasm wrapper — because replay interleaves with storage I/O, and a wrapper could only do that by importing I/O capabilities, breaking purity and browser portability. (3) Wrapper components reserved for pure middleware (enrichment, upcasting, validation).

**A4:** Didn't understand the question as asked — too much jargon. Re-asked in plain
language (see Q4-restated).

---

**Q4 (restated):** Plain-language version. (1) The author's module is shaped like a *class*:
create a session (optionally from snapshot) → feed it events in batches → ask it to decide
→ optionally dump state bytes for a snapshot. Not three loose functions that ship state
bytes in/out on every event. (2) The surrounding loop — read stream, drive the session,
append results with concurrency check — is platform code in the host (Rust service + JS
browser lib), not inside a wasm wrapper, because a wrapper doing I/O would need imports =
capabilities = trust problem again. (3) Wasm-wrapping-wasm stays, but only for pure jobs
(e.g., upcasting old event formats). Confirm or push back?

**A4 (restated):** User couldn't evaluate the technical internals ("I don't know how to
answer you"). Resolution: per A3's explicit delegation ("you tell me, what should we do"),
Claude made the call — session resource + batched evolve, zero imports, host-owned replay,
wrappers for pure middleware only. Recorded as DECIDED in `WASM_TODO.md`; user veto stands,
to be exercised once there's something concrete to see. Lesson applied: questions to the
user now target vision/goals (answerable from their seat), not component-model internals.

---

**Q5:** Who do you imagine writing these decider modules over time — (a) just you / your immediate team, (b) other teams across the org, (c) eventually third-party or community authors?

**A5:** Everyone, eventually — the full spectrum, including trusted AND untrusted
environments/authors. First-party workflow: create a pull request → merge → CI builds the
artifacts → deploy, and so on.

---

**Q6:** What do you picture actually happening in the browser — (a) real apps running the same decider client-side (optimistic validation before the command reaches the server), (b) a playground/simulator for authors (paste events + a command, watch decide/evolve — testing, demos, debugging), (c) local-first apps where the browser executes against locally synced events, (d) several of these / something else?

**A6:** Browser = running unit tests of these modules. Same for AI sandboxing — AI agents
running/testing modules inside their sandboxes. Possibly further: allow the AI to load
modules directly — maybe even a store — i.e., an AI agent hosting deciders plus an
in-memory event store to simulate the system safely.

---

**Q7:** First visible proof — (a) scheduler decider compiled to wasm, passing its existing given/when/then tests through the new sim host (no infrastructure); (b) platform slice: command on NATS → wasm module → events in JetStream; (c) browser/AI angle: sim host in a browser page or as an MCP tool?

**A7:** "Ideally I should be able to take a YAML file or something like that, a WASM
compiled module, and run the test with it." → Milestone 1 = a generic test runner: load a
compiled module + a YAML given/when/then suite, execute, report. Tests are data, not code.

---

## Session complete (A1–A7)

Conclusions distilled into `WASM_TODO.md` (goals, decisions 1–9, Milestone 1).
Next: Claude drafts the technical proposal (WIT package, crate layout per ADR 0002,
milestone plan) for review — no code until approved.

---

## Follow-up: "What are those resources and why do we use them?"

A WIT **resource** is an object that lives *inside* the module; the host only holds an
opaque handle to it (like a coat-check ticket or a Unix file descriptor) and makes calls
against the handle. Our design has exactly one: the `session`.

Why: host and module have separate memories — without a resource, the whole state would be
serialized and copied across the boundary on *every* replayed event, and every author in
every language would have to write that state codec. With the `session` resource, state is
created in-module, mutates in place, and never crosses; only event/command bytes travel.
State serializes exactly once — when (if) the host requests a snapshot.

Fit to goals: polyglot DX (a resource = "a class" in any language, no serialization
homework), performance (only per-event cost left is the unavoidable payload decode), trust
(handles can't be forged; fresh session per command execution, dropped afterwards — no
state lingers between runs).

---

## Follow-up: "Where does the loop (snapshot, reading, etc.) live — host or guest?"

**Host, entirely.** The guest is passive — it only answers calls. Host does: receive
command → load snapshot bytes → read event batches → drive `session.evolve` → call
`session.decide` → append with OCC (+ retries) → optionally request + store a snapshot.
Guest does: answer `stream-id`, decode its own snapshot, fold events, decide, serialize
state on request.

Why: (1) capabilities — the I/O steps need connections the zero-import guest cannot have;
(2) continuity — this loop already exists as `CommandExecution` in
`trogon-decider-runtime`; only `evolve`/`decide` move into the sandbox.

Nuance: snapshot **policy** (when) = host, uniform across modules; snapshot **content**
(bytes) = guest-owned, host stores blob + stream position without interpreting it.
Two host embodiments share the loop: platform service (JetStream + KV) and sim host
(in-memory; YAML runner / browser / AI sandboxes).

---

## Follow-up: "What's the real-world penalty of host↔guest crossings? Can we get 1ms for 1–10 events?"

Per crossing: ~0.1–1µs (component-model call + byte copy). Per command with batching:
~4–6 crossings + ~10–50µs fresh instance (pooled) → **total sandbox tax ≪ 100µs** for
1–10 events. Sub-millisecond: YES for decider execution (sim host runs scenarios in tens
of µs). NO as an end-to-end production guarantee — JetStream read + OCC append costs
~1–5ms in RTTs and dominates, exactly as it already does for native deciders; WASM doesn't
move that needle. SLOs split accordingly: execution SLO (p99 < 200µs, ≤10 events) under an
end-to-end SLO (low single-digit ms). Hazards: cold compile (→ precompile at
registry-push), long streams (→ snapshots), fuel metering (→ epoch interruption for
trusted tier). M1 gains a criterion benchmark (1/10/100/10k events) to validate with real
payloads. Full table: `WASM_TODO.md` § Performance expectations.

---

## Follow-up: "Where is the initial state?"

Inside the guest, absorbed into the session constructor: `constructor(none)` → guest calls
its own `initial_state()` (the author still writes it exactly as today, e.g.
`create_schedule.rs` delegating to `state::initial_state()`); `constructor(some(bytes))` →
guest decodes its snapshot instead. It is NOT exported to the host: the Rust trait only
exposes `initial_state()` because the host-side fold needed an in-process starting value —
in the resource design the fold lives inside the guest, so the starting value never
crosses. Exporting it would force state serialization (the codec burden we eliminated) and
give the host bytes it cannot interpret. Host worldview: state is invisible; it only
surfaces as opaque snapshot bytes on request.

---

## Follow-up: "We do need the write precondition for sure"

Confirmed as a hard requirement. Design (decision 10 in WASM_TODO.md): a static
`descriptor()` export listing every command-type with `option<write-precondition>`
(any / stream-exists / no-stream; none → host's observed-position OCC default). Mirrors
layer 1 of today's precedence chain (execution.rs:562: decider const → caller override →
position default); layers 2–3 stay host-side. Host fetches the descriptor once at module
load and caches it. The guest only *declares* — JetStream's expected-sequence does the
enforcement, so a module can't bypass concurrency control. The command list also gives the
platform routing inventory + an M1 conformance check (YAML suite covers all commands).

---

## Follow-up: "Share the event codec so other wasm (event handlers) can bundle it?"

Two levels (decision 11 in WASM_TODO.md): same-language sharing = the generated proto
library, statically linked (exists today). Cross-language/bundling = a **codec library
component** per stream type (`trogon:schedules-codec`) with WIT types generated from
proto, exporting typed `decode`/`encode`; decider + event-handler worlds import it and CI
composes it in (`wac plug`). Purity survives composition; conformance gate stays "zero
unresolved imports". Same-digest composition gives bit-identical decode in both modules.
The codec component also becomes the single home for upcasting (v1+v2 → current), and can
carry the json testing interface so YAML suites cover event handlers too. Event-handler
world itself is a future milestone.

---

## Follow-up correction: "No upcasting ever! They upcast themselves explicitly."

User CORRECTED the previous entry's upcasting idea — upcasting is **banned everywhere**
(decision 12 in WASM_TODO.md). No transformation between stored bytes and the fold: not in
the host, not in wrappers, not in the codec component. The codec's decode is
version-faithful — one variant case per stored version (`created-v1`, `created-v2`, …),
never converting. Each consumer handles every live version explicitly in its own
evolve/handle; exhaustive matching makes the compiler enforce it when a new version case
appears. Rationale: an upcaster is a hidden function whose change silently alters what
historical streams replay to; explicit per-consumer handling keeps replay-behavior change
inside reviewed module code. Accepted cost: explicit arms for all live versions, forever
(until streams are deliberately rewritten/archived).

---

## Follow-up: "Reuse the event marshaller as a wasm module embedded into another?"

Yes — this is decision 11 level 2, sharpened (see WASM_TODO.md § Marshaller-as-embeddable-
component). Key points: (1) the codec component becomes the DEPLOYABLE event contract —
one `schedules-codec.wasm` artifact instead of shared `.proto` + per-team codegen; proto
stays source. (2) Layering keeps decisions 1/5 intact: codec composition is INTERNAL to
the host's view — host ↔ decider stays opaque bytes, decider ↔ codec is typed events over
the canonical ABI hidden inside the composed artifact; `wac` satisfies the import so the
final module still has zero unresolved imports. (3) Two embed modes: static link
(zero-copy, same-language, default for Rust) vs component composition (one structural copy
per event, any language, shared-by-digest) — composition is for cross-language reuse, not
speed. (4) Still version-faithful (no upcast); typed variant is a pinned WIT contract
forcing recompile on schema growth; unknown events get an explicit outcome the consumer
resolves. Granularity: per-stream-type, not per-event-type.

---

## Follow-up: "Can I no-op events I don't need to support?"

Yes — but explicit per-event no-ops only, never a catch-all `_ => state` (decision 13).
The catch-all silently swallows future events = the hidden behavior decision 12 banned.
Role matters: read-side handlers/projections may no-op (skip) freely; the write-side
decider must NOT no-op events that feed the state `decide` reads — dropping a real
transition makes it decide on false state (e.g., handle only `CreatedV1`, no-op a later
`CreatedV2` creation → state seen MISSING → duplicate create). Unknown events (not in the
codec variant): read-side skips, write-side fails closed / fences (incomplete state ⇒
refuse to decide). Recorded as decision 13.

---

## Follow-up correction: "Each decider can be its own thing, sometimes."

Corrects the earlier "one module = one stream type" framing (decision 14). The Decider
trait is per-command, so the atom is the single decider; bundling is optional. Invariant: a
module = one consistency boundary (one State/Event/stream = one session); commands inside =
1..N, author's choice. WIT contract unchanged. Shapes: standalone (N=1, independent deploy
— often the better grain, serves goals 1 & 3); bundle (N>1 sharing state, e.g. scheduler's
4); split-but-shared-state (separate modules sharing a state) — ALLOWED only if the fold
(`evolve`) + state + codec are ONE shared composed component, never duplicated, because
divergent evolve = divergent replay = corruption (decision 12 spirit). v1: one session per
module.

---

## Follow-up: "Could the types come from buffa?"

Decision 17. Findings: buffa is EXTERNAL (crates.io 0.7.0), codegen emits checked-in
`gen/*.rs`; the schema also has `elixirpb` annotations → multi-language codegen from one
proto source already exists. So: not from buffa-the-crate (no WIT backend, not ours), but
from the SAME proto source — WIT as the Nth codegen target (checked-in `gen/*.wit` beside
`gen/*.rs`), exactly like Elixir already is. Tiers: type strings + FileDescriptorSet free
today (v1, routing + test JSON); typed WIT domain types via a new proto→WIT emitter
(post-v1, must map proto open enums → explicit `unknown` per decisions 11/13, plus
presence/maps/well-known/recursion); protocol WIT types (any-envelope/session/descriptor)
hand-written, not derived. Host↔module boundary stays opaque bytes regardless (generic
host); generated typed WIT lives only at the internal module↔codec seam. v1 ergonomic win:
the guest macro derives type strings + dispatch from buffa reflection, so `export_decider!`
only lists which structs are commands.

---

## Follow-up: "I'm still not sure how to make WIT and protobuf friendly at the language level."

Decision 16. Resolution: WIT and protobuf NEVER meet as types — only as bytes. wit-bindgen
owns envelope/session/error types; buffa owns domain types; they touch only at
`payload: list<u8>` ⇔ `Vec<u8>`. No tool needs both type systems (this is exactly what the
opaque-bytes boundary, decisions 5/15, buys). The bridge already exists in the repo:
`EventType` (→ type string), `EventEncode` (proto→bytes), `EventDecode` (bytes→proto, with
`Skipped` for foreign = decision 13), plus `buffa::Message` (v0.7.0, json feature on) for
snapshots. The guest SDK macro reuses these to implement the wit-bindgen `GuestSession`
trait, wrapping the author's UNCHANGED `impl Decider`. One crate runs cargo-component
(wit-bindgen) + buffa codegen side by side. Polyglot: same pattern, each language uses its
native proto lib on the payload — no proto↔WIT bridge. The ONLY place needing a real
proto→WIT generator is the typed cross-language codec component — deferred past v1.
Spikes queued: buffa × cargo-component build; buffa dynamic/JSON surface for the generic
test runner.

---

## Follow-up: "Any is for the unit-test YAML; keep a general any-envelope for most things, distinct from command-envelope."

Scope correction (decision 15). `google.protobuf.Any` canonical JSON (`@type` +
proto3-JSON) is the encoding for the TEST YAML files — NOT a rewrite of the whole WIT wire
boundary into the literal proto type (my first pass over-extended). Test runner does
proto3-JSON ⇄ binary via a standard `FileDescriptorSet` (e.g. prost-reflect), descriptor
set from the first-party proto build; no custom `encode-json` guest export. WIT boundary
keeps a general `any-envelope { type, payload }` for events & most payloads, with
`command-envelope` as its own distinct type. Both Any-shaped WIT records, not the proto
well-known type. Host routes on `type` only (decision 5 intact). Resolves the open M1
encoding question.
