# TODO: Runtime-enforced event headers

## Decision

Keep the storage/wire envelope generic:

- `event_type`
- payload bytes
- headers
- store metadata

Do not add causality, occurrence time, or correlation fields to domain event payload protos.
Do not introduce a domain-facing `EventContext` proto for this.

The required contract should be enforced in the runtime type system and encoded as well-known event headers.

Keep `trogon_decider::Decider` unopinionated. The pure decider contract should remain:

```text
state + command -> domain events
```

The header requirement belongs in a runtime composition layer, not in the decider trait and not in every domain payload.

## Current state

- `trogon-decider-runtime::Event` already separates `content` from `headers`.
- `EventEncode` already owns payload serialization only.
- `EventType` already lets command execution derive the stored event type from the typed event.
- `CommandExecution` already encodes typed events into `Event` envelopes, but currently defaults to `Headers::empty()` and accepts raw `Headers` through `with_headers`.
- `trogon-decider-nats` already stores event metadata in NATS headers and encodes user event headers with the `Trogon-Header-` prefix.
- `StreamEvent` already returns the stored `Event` envelope plus `recorded_at`, so readers can access event headers without changing payload decoding.
- Scheduler `Message.headers` are delivery-message headers, not event causality headers. Keep those concepts separate.

## API direction

Prefer composition over coupling.

Do not add actor identity, occurrence time, correlation, or header requirements to the `Decider` trait.

Add a composition point around event envelope construction, for example:

```rust
trait EventHeaderPolicy<C: Decider> {
    type Error;

    fn headers_for(&self, command: &C, event: &C::Event) -> Result<Headers, Self::Error>;
}
```

The command runtime then composes independent responsibilities:

```text
typed event
  -> EventType derives event_type
  -> EventEncode derives payload bytes
  -> EventHeaderPolicy derives validated headers
  -> EventId generator derives event id
  -> Event envelope
```

The base runtime can keep low-level APIs for tests, imports, migrations, and adapters.
The product-facing stack should expose a stricter facade where normal command execution is impossible without a validated header policy or typed required-header value.

Possible public shapes:

```rust
CommandExecution::new(store, command)
    .with_event_header_policy(policy)
    .execute()
```

or:

```rust
StrictCommandExecution::new(store, command, required_headers)
    .execute()
```

This keeps the decider reusable while letting applications opt into the stronger event-header contract.

Use `with_event_header_policy` for the composable base runtime API. The `with_` prefix matches the existing builder style on `CommandExecution` and makes the policy read as an added runtime concern. If a stricter product facade requires the policy at construction time, that facade can take the policy in `new`.

## Work required

### 1. Define the well-known event header contract

Align with CloudEvents (CE). Each well-known event header maps to a documented CE
attribute or extension. Reference:
[CE spec](https://github.com/cloudevents/spec/blob/main/cloudevents/spec.md),
[CE extensions](https://github.com/cloudevents/spec/tree/main/cloudevents/extensions).

| Concern             | Header name      | CE source                  | Required on normal command path | Type / format |
|---------------------|------------------|----------------------------|---------------------------------|---------------|
| Occurrence time     | `time`           | CE core                    | required                        | RFC 3339 timestamp. When the occurrence happened. |
| Recorded time       | `recordedtime`   | CE Recorded Time extension | set by store, not by caller     | RFC 3339 timestamp. SHOULD be ≥ `time`. |
| Workflow id         | `correlationid`  | CE Correlation extension   | required (runtime-generated if caller omits) | Non-empty string. Groups events in one logical flow. |
| Causing event id    | `causationid`    | CE Correlation extension   | optional                        | Non-empty string. SHOULD be the `id` of the directly causing event. |
| Principal type      | `authtype`       | CE Auth Context extension  | required                        | Enum: `app_user`, `user`, `service_account`, `api_key`, `system`, `unauthenticated`, `unknown`. |
| Principal id        | `authid`         | CE Auth Context extension  | required                        | Non-empty string. Avoid PII; prefer stable identifier or hash. |
| Acting on behalf of | `onbehalfof`     | No CE counterpart (custom) | optional                        | Non-empty string. Single custom extension, documented alongside the others. |

Wire encoding stays as today: stored on the NATS message with the existing
`Trogon-Header-` prefix. The names above are the canonical attribute names; the
wire prefix is a transport concern.

**Translation to CloudEvents wire format.** CE's HTTP binding standardizes a
`ce-` prefix (`ce-time`, `ce-correlationid`, etc.) and other bindings have their
own mappings. We keep `Trogon-Header-` internally because this is our stack,
not an external CE bus. If/when we bridge to a CE-aware consumer or producer,
add a translation layer at the boundary that rewrites prefixes both ways. The
attribute names themselves already match CE, so the translation is purely the
prefix swap plus any binding-specific quoting rules — no semantic mapping
required. Do not expose `Trogon-Header-` names in any external-facing API.

#### Caveats

- **`causationid` points at an event `id`, not a command id.** If we need
  "which command produced this event," that is a separate custom attribute, not
  `causationid`.
- **`authcontext` is purely informational.** Do not use `authid` / `authtype`
  for authorization decisions. Real auth still lives elsewhere; these are audit
  metadata.
- **Cron-fired events use `authtype = system`.** Scheduler ticks have no human
  principal; the CE enum already covers this case.
- **`recordedtime` is set by the store on append, not by the caller.**
  Bitemporal: `time` is when the occurrence happened, `recordedtime` is when
  this record was written. `StreamEvent.recorded_at`
  (`trogon-decider-runtime/src/event/stream_event.rs:18`, populated in
  `trogon-decider-nats/src/stream_store.rs:517`) is already this concept.
  Backfills, replays, and offline-then-synced commands drift these two apart;
  projections that ask "what was true at time T" want `time`, operational/audit
  questions like "when did the store see this" want `recordedtime`.
- **Distributed tracing is a separate concern.** CE's `traceparent` /
  `tracestate` (Distributed Tracing extension, W3C Trace Context) is for OTel
  spans, not for business correlation. Do not fold it into `correlationid`.

#### `authtype` value guide

The `authtype` enum is defined by the
[CloudEvents Auth Context extension](https://github.com/cloudevents/spec/blob/main/cloudevents/extensions/authcontext.md).
It is **not** an IETF RFC — it is a CNCF spec, and the values are pragmatic
categories drawn from cloud-provider IAM models. The spec allows
implementations to define additional values; we currently don't need any.

| Value             | Meaning                                                                 | Our use cases                                                |
|-------------------|-------------------------------------------------------------------------|--------------------------------------------------------------|
| `app_user`        | End user of the application, authenticated through a product-layer IdP. | Customer signed into UI/API. Identity managed at product layer. |
| `user`            | Human registered in the infrastructure layer (IAM/SSO), not the app.    | Engineer on CLI, ops/admin acting outside normal product flow. |
| `service_account` | Named, configured non-human principal representing an internal service. | Service-to-service calls, workers consuming queues, CI jobs.   |
| `api_key`         | External non-human principal authenticated by an opaque key.            | Third-party integration. `authid` is the key id/hash, never the key itself. |
| `system`          | Obscured principal — the platform or automation triggered the event.    | Cron ticks, retention/TTL expirations, replay/migration tools. |
| `unauthenticated` | No credentials were presented.                                          | Public webhook intake, deliberately open endpoints.            |
| `unknown`         | Principal type could not be determined.                                 | Legacy/external bridge ingestion only. Not for the normal path. |

Distinctions worth preserving:

- **`app_user` vs `user`** — who manages the identity: product vs infrastructure.
  Admin tooling acting on an IAM-layer identity should not read as a product user.
- **`service_account` vs `system`** — a service account is a *named, deliberately-
  configured* internal principal; `system` is anonymous platform action with no
  real actor behind it.
- **`service_account` vs `api_key`** — service accounts are typically internal;
  API keys are typically issued to external parties.
- **`unauthenticated` vs `unknown`** — `unauthenticated` is "we know there were
  no creds", `unknown` is "we failed to determine".

`unknown` must not appear on the normal command path. If it shows up regularly,
the upstream source needs fixing.

#### Open header-naming questions resolved

- Canonical names: use CE attribute names verbatim
  (`time`, `recordedtime`, `correlationid`, `causationid`, `authid`,
  `authtype`), plus a single custom `onbehalfof`.
- Timestamp format: RFC 3339 (CE requirement for `time` and `recordedtime`).
- Actor id format: opaque non-empty string; producer chooses stable id or hash.
  Spec explicitly warns against embedding PII.
- `correlationid` policy: required on the normal path, runtime-generated when
  the caller omits it (matches CE best practice "generate at entry point").

### 2. Add typed runtime headers

Split mechanism (header-agnostic) from vocabulary (specific header set):

**Mechanism — in `trogon-decider-runtime`:**

- `EventHeaderPolicy<C: Decider>` trait — runtime composition hook
- `CommandExecution::with_event_header_policy(...)` builder method
- Pre-append validation (see §4) — validates whatever the policy declares
  required, not specific header names
- `Headers` low-level map stays as today

**Vocabulary — in the scheduler / cron crate for now:**

- `SchedulerEventHeaders` value object with the CE-aligned fields from §1
  (`time`, `correlationid`, `causationid`, `authid`, `authtype`, `onbehalfof`)
- Canonical header name constants
- `impl EventHeaderPolicy<SchedulerDecider>` that builds and validates the set
- `TryFrom<&Headers>` for projection-side parsing

This type should:

- require the non-optional headers at construction time
- validate header names and values
- convert into the existing low-level `Headers`
- parse from low-level `Headers` for readers and projections
- keep extra headers possible without weakening the required contract

The existing `Headers` map should remain the low-level envelope representation,
not the normal application API.

Promote the vocabulary type to a shared crate when a second domain needs the
same fields. Name it `SchedulerEventHeaders` (not `RequiredEventHeaders`) so the
move is non-breaking when it happens.

### 3. Add a composable execution policy

Add a runtime composition point that can supply and validate headers during event envelope construction.

Possible implementation directions:

- add `with_event_header_policy(policy)` to `CommandExecution`
- add a stricter product-facing execution facade that wraps `CommandExecution`
- add a typestate builder where `execute` is only available after a header policy or required headers are supplied
- keep `with_headers(Headers)` as a low-level escape hatch for tests, imports, migrations, or adapter-level work

The pure `Decider` trait should stay focused on:

```text
state + command -> domain events
```

Headers belong at the runtime execution boundary, not inside every domain event payload.
The normal product path should make empty or malformed required headers impossible or fail before append.

### 4. Validate before append

Before `CommandExecution` builds the final `AppendStreamRequest`, validate that every event envelope has the required well-known headers.

Storage adapters can also defensively validate, but the main enforcement should happen before append in the runtime path.

Legacy or import paths need an explicit policy:

- reject missing headers
- allow missing headers only through a named migration/import API
- synthesize system headers for old events

Do not make missing headers silently acceptable on the normal command path.

### 5. Preserve payload correctness through typed events

Keep payload correctness separate from header correctness.

The normal path should continue to derive:

- `event_type` from `EventType`
- payload bytes from `EventEncode`

Avoid public application paths that accept arbitrary `(event_type, payload_bytes, headers)` unless they are clearly low-level migration or adapter APIs.

Consider adding an `Event` constructor or factory that builds envelopes from:

```text
typed event + required headers + event id generator
```

That would reduce direct struct construction and make mismatched event types and payloads harder to create.

### 6. Keep proto changes minimal

Do not add event causality fields to scheduler payload protos.

Do not add a domain-facing `EventContext` proto.

If append/read APIs are represented in proto, make sure the envelope carries headers as headers. The runtime can still provide typed construction and validation above that wire shape.

Scheduler delivery message headers remain scheduler delivery data. They should not be reused as event causality headers.

### 7. Expose typed headers to readers and projections

Readers and projections should be able to parse the well-known headers from `StreamEvent.event.headers`.

Add helper APIs so projection code does not manually lookup strings everywhere, for example:

```text
stream_event.required_headers()
```

or:

```text
RequiredEventHeaders::try_from(&stream_event.event.headers)
```

Payload decoders should stay payload-focused. They should not need headers unless a projection or business rule explicitly needs event metadata.

### 8. Migrate existing scheduler event payloads later

Do not remove current scheduler or CRON `*_by` / `*_at` payload fields just because headers exist.

Migration order:

1. Add and enforce required runtime headers for new appends.
2. Ensure NATS round-trips the well-known headers.
3. Expose typed header parsing to readers and projections.
4. Dual-write duplicated payload fields and headers where needed.
5. Move consumers to headers when the field is generic causality or occurrence time.
6. Remove duplicated payload fields only after consumers no longer depend on them.

Fields that are real business facts stay in payloads.

## Tests to add

- command execution fails before append when required headers are missing
- command execution fails before append when required header values are malformed
- command execution with typed headers appends events with the expected low-level headers
- event type and payload are still derived from the typed domain event
- NATS append/read round-trips the well-known event headers
- projections can parse required headers from `StreamEvent`
- legacy/migration behavior for missing headers is explicit and tested

## Open decisions

Resolved (see §1):

- ~~Exact canonical header names.~~ Use CE attribute names verbatim.
- ~~Whether `correlation_id` is caller-required or runtime-generated when omitted.~~ Required on the normal path, runtime-generated when caller omits.
- ~~Actor id string format.~~ Opaque non-empty string, no PII.
- ~~Timestamp format for `occurred_at`.~~ RFC 3339 (CE requirement).

Still open:

- Whether low-level raw `Event` construction should remain public or move behind constructors.
- Whether all events emitted by one command share the same headers, which matches the current `CommandExecution` shape.

### Custom `commandid` attribute

CE `causationid` links event → event only. The first event in any flow has no
causing event — it was caused by a **command**, and CE has no slot for that.
Cases where this matters:

- **Command idempotency / retry dedup.** Scheduler delivers at-least-once; the
  same command can land twice. Projections need the command id on emitted
  events to answer "did I already produce events for this command?" without
  fuzzy timestamp/payload matching.
- **Audit root.** Walking `causationid` backwards through events dead-ends at
  the first event. The honest root is "command X fired by Y at T".
- **Replay correctness.** Replaying a command log alongside an event log
  requires the command-id linkage; CE causation only links events.
- **Cron-specific.** Every tick is a command. "Which tick produced these
  events" is a routine debugging question.

Hack alternative: share an id namespace between commands and events and stuff
the command id into the first event's `causationid`. Works, but conflates two
concepts in one slot. Decision: add a separate custom `commandid` attribute.
Open: exact name, whether it applies only to the first event in a flow or
every event.

### `onbehalfof` (custom) vs CE `authclaims`

CE Auth Context defines `authclaims` as a JSON string of arbitrary principal
claims. Two ways to represent delegation:

- **Custom `onbehalfof` attribute** — one wire slot, one string. Typed, fast to
  read. Cost: non-CE-standard; future CE bridges need a mapping.
- **CE-standard `authclaims`** — encode `{"on_behalf_of": "...", ...}` as JSON
  in the standard `authclaims` slot. CE-aligned, one slot covers arbitrary
  future claims. Cost: readers JSON-parse to get one field.

Decision driver: frequency of delegation. Rare → fold into `authclaims`,
smaller wire surface. Common (admin impersonation, service-on-behalf-of-user
patterns) → typed `onbehalfof` pays off in read ergonomics. Open.

### OTel propagation ownership

CE Distributed Tracing extension defines `traceparent` / `tracestate` and
requires they carry the **starting** trace context of the transmission. Who
stamps them?

1. **Caller stamps.** Each call site extracts the current OTel span and passes
   `traceparent` via the policy. Clean separation. Failure mode: easy to
   forget; silent gaps in traces.
2. **Runtime stamps.** `CommandExecution` reads ambient OTel context at append
   time and writes `traceparent` itself. No call site has to remember. Failure
   mode: couples `trogon-decider-runtime` to the OTel SDK; misbehaves for code
   paths outside any active span.
3. **Policy stamps.** Each domain's `EventHeaderPolicy` decides — cron policy
   pulls from the scheduler tick span, HTTP-handler policy pulls from the
   request span. Most flexible. Failure mode: per-domain boilerplate, easy for
   two domains to diverge.

For cron alone, #2 is simple — every tick is inside a scheduler span. For a
generic runtime, #3 is more honest. Open.
