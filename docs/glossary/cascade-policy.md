---
term: "Cascade policy"
section: "Agent execution model"
order: 17
---

# Cascade policy

The recorded rule governing a [child session](./child-session)'s fate when its
parent reaches a terminal state: `CASCADE_ON_PARENT_TERMINAL` (the safe
default -- the child is cancelled, preventing a leaked orphan) or `INDEPENDENT`
(an intentional, recorded orphan that keeps running). The safe default is
applied at the command boundary, at the application level, before append; the
protobuf zero value (`CASCADE_POLICY_UNSPECIFIED`) is rejected at validation
and is never persisted. Recording the choice as an explicit fact closes the
silent-orphan gap the studied agent products left open. See
[ADR#0035](../adr/0035-session-store-decider-aggregate.md).
