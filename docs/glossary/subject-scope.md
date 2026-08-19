---
term: "Subject scope"
section: "Event sourcing and the decider"
order: 15
---

# Subject scope

The NATS subject subtree a `StreamSubjectResolver` declares every subject it
resolves will fall inside. Defined by
[ADR#0027](../adr/0027-decider-multi-tenancy-primitive.md) and implemented as
`trogon_decider_nats::SubjectScope`.

The scope is fixed when the resolver is constructed and the subject is derived
per call, so `JetStreamStore` comparing them is a real check rather than a
resolver agreeing with itself: a resolver that composes the wrong prefix is
refused before the read or the append. Declaring a scope is optional, and a
resolver that declares none is not checked.
