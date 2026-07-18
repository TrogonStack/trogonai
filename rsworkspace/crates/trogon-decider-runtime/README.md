# trogon-decider-runtime

The `-runtime` suffix (per [ADR#0002](../../../docs/adr/0002-rust-crate-boundaries.md)) marks
execution machinery built on top of the domain crate; this crate owns the storage-neutral
contracts a backend adapter implements (`StreamRead`, `StreamAppend`, `SnapshotRead`,
`SnapshotWrite`) and the runtime boundary that drives one command through them:
`CommandExecution`. `CommandExecution` reads a snapshot (if configured) and stream history,
folds it through `Decider::evolve`, calls `Decider::decide`, and appends the resulting events,
normalizing every phase's failure into one `CommandError`. It also owns snapshotting policy
(`SnapshotPolicy`, `SnapshotFailurePolicy`), event headers (`Headers`, distinct from event
payloads), and stream position semantics (`StreamPosition`, `ReadFrom`).

This crate has no dependency on any specific storage backend; `trogon-decider-nats` is one
adapter that implements its traits against NATS JetStream, and `#[cfg(feature =
"test-support")]`'s `InMemoryStore` is another used only in tests.

See `src/lib.rs` for a runnable example, and
[Decider Platform](../../../docs/architecture/decider.md) for the full `CommandExecution` flow,
including snapshot failure recovery and the header/ADR#0013 interaction.
