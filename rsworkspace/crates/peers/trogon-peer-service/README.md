# Peer enrollment service

This crate applies communication enrollment and connector resume through the
shared decider runtime. Its caller supplies a store, an authorizer for `Enroll`
and `Attach`, and a `PeerClock`; the service provides no default policy or clock.

The transport boundary must derive `TrustedPeerOwner` from the admitted subject
principal before calling `enroll`. A request cannot select its tenant or caller.
The selected connector profile uses an `Agent` command principal whose ID is the
trusted caller ID. The injected policy must authorize the owner, installation,
and collaboration encoded in the typed command.

Request fields become validated domain values before execution. The runtime
authorizes before reading history, then replays, decides, and appends the core
events under its guarded write contract. Initial enrollment and attachment
replacement return the state after the append succeeds. Repeating enrollment for
the same live connector returns its existing attachment without appending;
another connector can replace it after its lease expires, fencing the previous
generation while preserving peer identity.

When create-only enrollment conflicts with an existing stream, resume authorizes
`Attach` before reading that stream. It validates the history, rejecting more
than 4096 events and any unknown event type, then pins the attachment append to
the validated stream position. A concurrent authority event rejects the append
instead of allowing a replacement across unvalidated history. The command replay
also retains the 4096-event limit. Reusing a live attachment rechecks history
before returning it.

Stores receive bounded read requests, but their implementation may materialize a
larger response before the service rejects it. The replay ceiling therefore does
not guarantee a transport I/O bound. A longer valid history cannot resume through
this boundary, and an event this version cannot interpret cannot be assumed
irrelevant to ownership or attachment authority. Longer-lived histories need an
authorized checkpoint or read-model contract; widening the bound or silently
skipping unknown events does not provide that contract.

Attachment renewal, presence queries, messaging, and network serving are separate
integration work at this layer.
