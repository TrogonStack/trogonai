# trogon-decider-nats

NATS JetStream storage adapter for `trogon-decider-runtime`. Owns the mapping from a decider's
logical stream identifiers onto one physical JetStream stream (subject-per-logical-stream, via a
caller-supplied `StreamSubjectResolver`), atomic multi-event append with optimistic concurrency,
ordered subject-filtered replay with bounded retry, and KV-backed snapshot storage
(`snapshot_store`). Per the crate boundaries in [ADR#0002](../../../../docs/adr/0002-rust-crate-boundaries.md),
this crate is the NATS-specific adapter layer; it implements `trogon-decider-runtime`'s storage
traits (`StreamRead`/`StreamAppend`, `SnapshotRead`/`SnapshotWrite`) but knows nothing about
`decide`/`evolve` or command execution itself.

It also owns two read-side primitives used to build projections and message-driven consumers on
top of decider streams: `Projector` (a generic catch-up driver for subject-filtered JetStream
projections, checkpointed via `ProjectionCheckpointStore`) and `Processor` (a durable pull-consumer
message loop with a `MessageHandler`/`HandlerVerdict`/`RedeliveryPolicy` contract). `provision`
provides idempotent `ensure_stream`/`ensure_bucket` create-or-open helpers used to set up the
underlying JetStream stream and KV bucket before either primitive runs.

See [docs/architecture/decider.md](../../../../docs/architecture/decider.md) for how this adapter fits
into the end-to-end decider platform, including when to reach for `Projector` versus `Processor`.
