# Usage Settlement Ledger

A billing consumer reads recorded usage and turns it into charges. It publishes
a charge and the acknowledgment never arrives. This page documents the ledger
that makes the next step safe. It covers the protobuf definitions that exist
today. There is no Rust implementation yet.

See [Session Aggregate](./session-aggregate.md) for where usage is recorded and
[Session Crash Boundaries](./session-crash-boundaries.md) for the general shape
of an operation whose outcome was never observed.

## Why this is not in the Session domain

Session records what was consumed. A billing consumer reads that and settles it.
The retry state of the second job is the consumer's problem.

Publishing settlement progress back into a session stream would put a downstream
system's bookkeeping into a log that is never truncated
([ADR#0035](../adr/0035-session-store-decider-aggregate.md) facet 7), so a
transient provider outage would leave permanent noise in the history of every
session it touched. The ledger is the consumer's own durable state, in its own
domain, at `proto/trogonai/usage/settlement/v1alpha1/`.

## The record is written before the charge

A ledger written after a successful publish cannot describe the failure it
exists to describe. If the process dies between publishing and recording, the
only evidence that a charge might exist is gone, and the retry has nothing to
consult.

So intent is durable first, and `SettlementState` is ordered around that:

| State | What a retry may assume |
| --- | --- |
| `UNSPECIFIED` | Nothing. Treat as `UNKNOWN`. |
| `INTENDED` | No charge exists. |
| `IN_FLIGHT` | A publish is running in this process. |
| `UNKNOWN` | A publish was attempted and its outcome was never observed. |
| `SETTLED` | The provider confirmed. Terminal. |
| `REJECTED` | The provider refused and said so. Terminal. |
| `ABANDONED` | Closed by a recorded decision. Terminal. |

`INTENDED` is the only state that lets a consumer conclude no charge exists, and
it holds only because the move to `IN_FLIGHT` is itself durable and happens
before the publish call. A crash cannot skip past it.

`IN_FLIGHT` is not durable in the sense a reader might expect. A consumer
recovering from a crash has no in-progress calls, so every record found in that
state on startup becomes `UNKNOWN`. Its useful life is one process lifetime.

The zero value is the one that blocks a blind retry, because the failure this
whole ledger prevents is a retry that assumes nothing happened.

`ABANDONED` is reachable only through an incident, so nothing in the automated
path can reach it. A consumer able to abandon a charge on its own is one that
quietly stops billing under sustained provider failure.

## The idempotency key comes from the usage, never the attempt

`settlement_id` is derived deterministically from the usage fact: session,
ordinal, meter. This is the single property that makes double charging
impossible rather than unlikely. A retry recomputes the same key from the same
inputs, so the provider sees the same request it may already have processed.

A key minted per attempt would make every retry a new charge, and the retry path
is precisely the path a lost acknowledgment forces a consumer down. That is why
`UsageRef` points at the ordinal that recorded consumption rather than at a
running total: totals are derived and can be recomputed differently, and the
fact that a specific ordinal recorded a specific quantity cannot change.

`ProviderReceipt.deduplicated` records when the provider reported a repeat under
the same key. It is worth keeping rather than discarding as a suppressed
success, because it is direct evidence that an earlier attempt landed and its
acknowledgment was lost. It is the difference between a system that is retrying
correctly and one that is about to be found out at reconciliation.

## The watermark and the ledger move independently

`ConsumerCheckpoint.processed_watermark` says how far the consumer has read.
The ledger says what those reads came to. They are tracked separately on
purpose: if the watermark could not advance past an unresolved settlement, one
charge stuck behind a provider outage would stop billing for everything after
it, and a short outage would become a backlog the size of the busiest tenant.

The watermark does have one ordering rule. It advances only after the ledger
records exist for everything below it. Advancing first would lose usage on a
crash, and that is the one failure this ledger cannot detect afterwards: an
unrecorded charge leaves nothing behind to reconcile against.

`unknown_settlement_count` is broken out from `open_settlement_count` because it
is the number that means money may have moved without anyone knowing, and
burying it in a general open count is how it stops being looked at.

## Recovery scans are bounded, and say what they missed

A recovery pass that scans the whole ledger gets slower exactly as the system
gets busier, so the pass meant to run during an incident is the one that stops
finishing during an incident.

`ScanOpenSettlementsRequest` bounds it three ways: by state, by idle time, and
by source position. `oldest_open_watermark` on the checkpoint is what makes the
third one useful, since it lets a sweep be proportional to what is open rather
than to the ledger's size. `idle_for` matters for the automated path
specifically: a settlement published a second ago is not stuck, it is in flight,
and a recovery pass that republishes it is competing with the attempt already
running.

`ScanCoverage` is always present, including on an exhaustive scan. An operator
reading an empty page needs to know whether nothing is open or the scan ran out
of budget, and those are identical without it. A non-zero `unreadable_count`
means unresolved settlements exist that this scan cannot name, so the sweep
cleared nothing.

## Reconciliation is a question, not a retry

`ReconcileSettlement` asks the provider what actually happened. It exists so a
consumer facing `UNKNOWN` has a move other than publishing again and hoping the
idempotency key holds, and no field on the request can cause a charge.

The outcome that matters most is `PROVIDER_CANNOT_SAY`, usually because the key
is older than the provider's retention for idempotency records. It is reported
separately from `CHARGE_ABSENT` because they differ on the only question that
matters: absent means republishing is safe, and cannot-say means the provider
has forgotten the key that was making it safe.

The zero value keeps the settlement unresolved, so a reconciliation returning a
variant the caller does not understand leaves the record needing attention
instead of closing it.

## Rewinding a session does not un-consume tokens

A session can be rewound, compacted, or redacted, and the ordinal that recorded
usage can leave effective history. The ledger deliberately does not follow.

Usage is not part of effective history. Tokens that were spent were spent, and a
ledger that tracked effective history would refund real compute every time a
user backed up a turn. What is genuinely ambiguous is whether a removal was a
correction or a privacy action, and that is a question for a person, so it
opens `INCIDENT_KIND_USAGE_RETRACTED` rather than reversing anything.

## Layout

`proto/trogonai/usage/settlement/v1alpha1/`:

| File | Contents |
| --- | --- |
| `ledger.proto` | `UsageRef`, `SettlementRecord`, `SettlementState`, `ProviderReceipt`, `SettlementIncident` |
| `recovery.proto` | `ConsumerCheckpoint`, the bounded scan, `ReconcileSettlement` |

No `service` definitions, matching the rest of this repo: transport binding is
JSON-RPC over NATS ([ADR#0055](../adr/0055-nats-subject-design-jsonrpc-bindings.md),
[ADR#0056](../adr/0056-canonical-jsonrpc-bodies-over-nats.md)).

## Status

Shipped: the two protos above, lint-clean, formatted, building, and generating
Rust bindings reachable at `trogonai_proto::usage::settlement_v1alpha1`.

Not shipped: the consumer, the ledger store, any provider binding, and the
incident workflow. The contract is the part that has to be right first, because
every mistake it prevents is a mistake that shows up on someone's invoice.
