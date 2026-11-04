# Pure peer message decisions

This crate decides enrollment, attachment, and exact-peer unicast on one guarded
history per peer. `PeerState` and `PeerEvent` are the generated contracts. Every
command requires `StreamUnchanged`, so attachment replacement contends with user
writes on the same history. There is no transport, content store, cryptography,
renderer, native hook, or workflow engine here.

## Service trust boundaries

The service supplies the current time, generated message and batch identities,
and authenticated `TrustedPeerOwner` and `AttachmentRef`. Resolve the principal
from the authorized bus subject as required by ADR 0082. Model input and headers
must not select that principal. Sender, preparation, and acknowledgment commands
check the owner, connector, generation, attachment status, and lease.

`TrustedContent::from_sealed_port` accepts metadata from a trusted sealed-content
port: an opaque reference, UTF-8 byte size, and 32-byte content digest. The port
must validate the actual text, compute its digest, seal it under ADR 0083, and
bind the reference to the authorized tenant and content. Token syntax, digest
length, and byte limits in this crate do not authorize content or prove sealing.
Do not construct this input from arbitrary tool-supplied references or digests.
No body bytes enter commands, immutable events, state, or their diagnostics.

`AcceptedMessage::from_sender_history` is the internal intake/disposition proof.
Load that history from the trusted tenant-scoped sender aggregate, never from a
user-provided state document. The factory checks the accepted entry against its
sender enrollment and semantic digest. Intake and disposition check its tenant,
exact recipient, immutable collaboration, and acceptance time. An offline or
expired connector does not remove the durable inbox.

`TrustedBatchPlan::from_renderer` is an internal renderer result containing
ordered message IDs and the total rendered UTF-8 size, including escaped
provenance and warnings. The renderer must validate complete rendered messages
before append. The core checks the oldest eligible intake prefix, current
eligibility, distinct IDs, at most four messages, the 8 KiB total limit, and a
body-size lower bound. It cannot independently measure text it never receives.
Rendered size is a decision input; the event stores assignment metadata only.
Persist the exact rendered presentation separately under the stable batch ID;
never truncate or rerender a persisted batch into different text on replay.

`AuthoritativeReceipt::from_recipient_history` is the internal settlement proof.
Load the authenticated recipient aggregate under its tenant. The factory checks
the disposition ordinal and its inbox/batch evidence. `SettleMessage` checks the
tenant, accepted message, recipient, outcome, revision, and decision time. These
internal factories and commands must not be exposed as user tools. No caller
supplied receipt is an authority merely because it matches the protobuf schema.

## Acceptance, ordering, and receipts

`SendMessage` records the key, semantic digest, and accepted metadata together.
The semantic digest includes contract version 1, sender, recipient,
collaboration, content digest, content size, and optional reply-to identity.
Proposed identity, time, and sealed-content location do not affect an exact
retry. The accepted identity, reference, acceptance time, and one-hour expiry
remain unchanged. A changed semantic request conflicts. Accepted entries remain
after settlement and content removal.

New sends reserve at most 16 unsettled messages and 32 KiB per sender. At most
10 new accepts fit in the rolling 60-second window, including accepts already
settled. Exact retries consume neither quota nor rate. Recipient intake reserves
at most 64 pending messages and 128 KiB. Overflow returns `DeferIntake` and leaves
the sender obligation unresolved. Intake ordinals give recipient-local order;
sender timestamps do not define global order.

Only the recipient history chooses preparation, expiry, or unavailability.
Expiry uses command decision time, with equality at the deadline expired; this
is not a promise about wall-clock time at the eventual physical append. Missing
recipient evidence cannot settle a sender. Expiry or unavailability before
intake creates a durable tombstone. The first disposition always wins.

`PrepareBatch` records the ordered batch and all preparation receipts in one
event. Each `recipient_revision` is the next positive aggregate disposition
ordinal, including each message in a prepared batch. It is not a physical broker
sequence, stream revision, or attachment generation. Receipts with unknown or
zero outcomes, zero ordinals, or inconsistent batch fields are rejected.

A prepared receipt means the recipient durably assigned the message to a batch.
It permits sender capacity settlement, while the recipient still counts that
message as pending. The oldest unacknowledged batch replays unchanged, including
after attachment replacement or later content expiry. An expired or unavailable
receipt means that terminal recipient disposition won instead of preparation.

`AcknowledgeBatch` stops presentation replay and releases recipient quota once.
It requires the recipient owner and current live attachment for an assigned
batch. It proves neither broker acknowledgment, host-context insertion,
comprehension, nor task completion. Sender settlement releases its quota once
without rewriting accepted metadata or replacing a winning outcome.

Typed `AlreadyAccepted`, `AlreadyIntaken`, `AlreadyDisposed`, `AlreadyPrepared`,
`AlreadyAcknowledged`, and `AlreadySettled` results carry the original metadata
through `MessageError`. The service should map these to idempotent responses;
they intentionally append no empty decision or replacement event.

Prepared text retention for 24 hours, privacy erasure, missing-evidence
reconciliation, sealed storage, and exact native delivery are service concerns.
Removing content must not rewrite message, batch, idempotency, or receipt history.

## Verification

From the Rust workspace, run `mise exec -- cargo test -p trogon-peer`.
The tests use deterministic clocks and public decider evaluation. They prove
pure decisions and replay invariants, not broker or native host delivery.
