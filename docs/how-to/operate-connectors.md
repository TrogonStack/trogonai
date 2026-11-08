# Operate Connector Recovery and Erasure

Use the [security profile](../reference/connector-security-profile.md),
[relay recovery contract](../reference/connector-relay-recovery.md) and
[custody identity rotation](../reference/connector-account-identity.md) as the
authority for an implemented deployment. Operate through authenticated service
workflows; do not edit OpenBao paths, stream records or alias keys by hand.

## Audit or replay-store outage

Close new admission when acceptance, receipt authority or outbox budgets cannot
be established. Drain durable completions and repair export sinks independently.
Reconcile acceptance/intent/completion by stable receipt identity. Before intent,
fence the owner and establish NOT_SENT; after uncertain intent/send, record
OUTCOME_UNKNOWN. Neither case clears the consumed nonce. Never repair an outage
by increasing TTLs, deleting tombstones or replaying provider writes.

## Restore or custody-key loss

Keep affected Sessions stopped. Fence old workers, invalidate grants and key
attestations, advance the independently durable restore epoch and reconcile
receipt/audit state. Token-material restore follows reconnect-required recovery;
it does not restore spend authority. Only a fresh admission/consent path can
establish new authority. An unproved epoch or key state remains unavailable.

## Account lookup-key rotation

Start before the configured cryptoperiod ends. Acquire the parent-scoped fence,
drain registry/claim appends and prove a complete authoritative coverage inventory.
Backfill next-generation aliases through custody using the existing erasable
profile or fresh trusted probe. Missing identity, aliases, keys or coverage
blocks cutover; it never allocates a second claim. Retire a computation key only
after the specified worker/receipt/backup barriers are satisfied.

## Disconnect and erasure

Commit the authorized disconnect/offboarding intent first. Observe provider
revoke separately from local material destruction and all profile revisions'
erasure. Revoke can be unsupported or rejected; key veto can make it impossible.
Report those outcomes honestly. Clear a claim only once its Connection is
terminal and reconcile cleanup debt until every required step has evidence.
A hold timeout is not a release authorization. Retention/hold policy applies to
safe audit metadata and exports, never as permission to retain raw profile data
outside its erasure workflow.
