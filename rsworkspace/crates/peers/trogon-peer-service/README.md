# Peer enrollment service

This crate applies initial communication enrollment through the shared decider
runtime. Its caller supplies a store, a `CommandAuthorizer<Enroll>`, and a
`PeerClock`; the service provides no default policy or clock.

The transport boundary must derive `TrustedPeerOwner` from the admitted subject
principal before calling `enroll`. A request cannot select its tenant or caller.
The selected connector profile uses an `Agent` command principal whose ID is the
trusted caller ID. The injected policy must authorize the owner, installation,
and collaboration encoded in the typed command.

Request fields become validated domain values before execution. The runtime
authorizes before reading history, then replays, decides, and appends the core
enrollment events under its guarded write contract. The response comes from the
state returned after the append succeeds.

This boundary handles initial enrollment only. An existing enrollment returns the
core decision error. Resume, attachment renewal, presence queries, messaging, and
network serving are separate integration work.
