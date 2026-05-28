# Understanding Event Context

This note captures the event-sourcing context decisions around actor identity, event metadata, headers, and event payload shape.

## Core Decision

Every persisted event record should carry a consistent context contract:

- what happened
- who caused it
- when it occurred
- how it relates to other events or commands

That context belongs outside the domain payload. The payload should describe the domain fact.

```text
RecordedEvent
  event_id
  event_type
  stream_id
  stream_version
  recorded_at
  context
    caused_by
    on_behalf_of
    occurred_at
    correlation_id
    causation_id
  payload
```

## Payload vs Context

Use the payload for business facts.

Use event context and record metadata for generic event-sourcing facts.

```text
payload
  job_id
  schedule
  grantee
  approver
  effective_at
  expires_at

record metadata
  event_type

event context
  caused_by
  on_behalf_of
  occurred_at
  correlation_id
  causation_id
```

Do not remove every actor or timestamp from payloads. Remove only the ones that mean generic causality or event occurrence time.

Keep actor or timestamp fields in the payload when they are part of the domain fact.

Examples:

- `caused_by` belongs in event context.
- `occurred_at` belongs in event context.
- `recorded_at` belongs to the event store record.
- `approved_by` stays in the payload if approval identity is part of the domain rule.
- `grantee` stays in the payload because it is the subject receiving a permission.
- `scheduled_for`, `starts_at`, and `expires_at` stay in the payload when they are business times.

## Occurred At vs Recorded At

`occurred_at` is producer, application, or domain time.

It answers: when did this event happen from the system that produced it?

`recorded_at` is event-store time.

It answers: when did this event store durably record the event?

For normal online commands, these values are usually close. They can differ for imports, retries, delayed jobs, backfills, offline writes, and queue processing.

```text
Command handled at: 10:00:01
Event stored at:   10:00:03

occurred_at = 10:00:01
recorded_at = 10:00:03
```

The caller supplies `occurred_at` through event context. The store assigns `recorded_at`.

## Actor Identity

Use `ActorId` for the canonical actor identifier carried in shared event context.

The value should be self-describing enough for replay, projection, and audit consumers to understand it without private service context.

Examples:

```text
users/usr_123
services/scheduler
systems/cron
integrations/github
```

Use `Actor` only when consumers need actor type and local id as separate fields.

`ActorId` already represents the actor-reference concept. A separate `ActorRef` type would duplicate that concept unless the system creates a truly different semantic role for references.

The field inside `ActorId` can remain `value` because the message name carries the role. Naming that inner field `ref` would not add a new concept; it would mostly create generated API and JSON churn.

## Headers

Headers are a good storage or transport encoding for event context.

Headers should not be the primary authoring API for core event-sourcing semantics.

The runtime should expose a typed value object such as:

```rust
pub struct EventContext {
    pub caused_by: ActorId,
    pub on_behalf_of: Option<ActorId>,
    pub occurred_at: DateTime<Utc>,
    pub correlation_id: Option<CorrelationId>,
    pub causation_id: Option<CausationId>,
}
```

The runtime can serialize that context into well-known headers.

```text
EventContext <-> well-known headers
Headers      <-> extra extensible metadata
```

That keeps NATS and storage adapters natural while avoiding magic strings in application code.

## Enforcing Who, What, and When

Do not rely on every event author remembering to set actor and timestamp fields.

Make context required at the command execution or append boundary.

```rust
pub struct CommandContext {
    pub caused_by: ActorId,
    pub on_behalf_of: Option<ActorId>,
    pub occurred_at: DateTime<Utc>,
    pub correlation_id: CorrelationId,
    pub causation_id: Option<CausationId>,
}
```

The decider should emit domain events.

```text
JobAdded
JobPaused
JobRemoved
```

The runtime should be the only layer that turns domain events into persisted records.

```text
DomainEvent + CommandContext -> RecordedEvent
```

Use these enforcement points:

- Require `CommandContext` for command execution.
- Derive `event_type` from the event type instead of passing arbitrary strings.
- Hide raw recorded-event construction from application code.
- Validate required context at append time.
- Use explicit system actors instead of missing actors.

Examples of explicit system actors:

```text
system:scheduler
system:migration
system:replay
external:stripe
external:github
```

## Current Runtime Caveat

The current runtime does not expose all of this context everywhere yet.

The existing shape has event content and headers, but decoded event data may only expose event type and payload. That means projections or deciders may not be able to read the full recorded-event context yet.

Because of that, do not remove existing `*_by` and `*_at` fields from current CRON or scheduler payloads first.

The safer migration is:

1. Add typed event context to the runtime API.
2. Store that context through the existing header mechanism or a first-class event field.
3. Make append and command execution require that context.
4. Expose context to readers, projections, and decode paths.
5. Dual-write current payload fields and event context during migration.
6. Remove duplicated payload fields only after consumers can rely on the event context.

## Naming Guidance

Prefer names that describe the role of the field in the event record.

- Use `caused_by` for the actor that caused the event.
- Use `on_behalf_of` when one actor acts for another.
- Use `occurred_at` for application/domain event time.
- Use `recorded_at` for store-assigned persistence time.
- Use `event_type` for the top-level "what".

Avoid a generic `actor` field in shared event context when the role is causality. `caused_by` is more precise.

Avoid putting generic `created_by`, `added_by`, `paused_by`, or similar fields in payloads when they only duplicate `caused_by`.

Keep those names in payloads when they are domain facts rather than generic causality.

## Practical Rule

Use this test when designing an event:

```text
Would this field still matter if the event were replayed from a different store,
transport, or projection?
```

If yes, and it is a business fact, keep it in the payload.

If yes, and it is generic causality, tracing, identity, or persistence context, put it in event context or record metadata.

If it is assigned by the store, keep it on the recorded event, not in the payload and not in caller-supplied context.
