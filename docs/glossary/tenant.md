---
term: "Tenant"
section: "Event sourcing and the decider"
order: 14
---

# Tenant

One customer whose decider streams and snapshots must stay isolated from every
other customer's.

The platform crates own no `Tenant` type.
[ADR#0027](../adr/0027-decider-multi-tenancy-primitive.md) resolved that the
tenancy vocabulary belongs to the consumer, and that what the decider crates
own is the storage-resolution half: a
[subject scope](./subject-scope) a `StreamSubjectResolver` declares and
`JetStreamStore` holds it to. A multi-tenant consumer defines its own tenant
value and projects it onto a scope.
