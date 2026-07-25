---
term: "Delegation"
section: "Agent execution model"
order: 16
---

# Delegation

The act by which a [session](./session) hands a scoped unit of work to another
party and awaits its result. Delegation to a platform-owned
[child session](./child-session) is recorded as a parent-side dispatch fact plus an
operation-ledger reservation that makes the dispatch idempotent; delegation to a
remote party the platform cannot attest is an
[external delegated agent](./external-delegated-agent) instead, with only a recorded
result and no child session. See
[ADR#0035](../adr/0035-session-store-decider-aggregate.md) and
[ADR#0031](../adr/0031-agent-implementation-and-session-plan.md).
