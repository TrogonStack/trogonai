---
term: "Decider"
section: "Event sourcing and the decider"
order: 1
---

# Decider

A plain Rust type implementing the `Decider` trait, holding domain decision logic
as the `decide` / `evolve` / `initial_state` cycle. It never performs I/O. See
[Decider Platform](../architecture/decider.md).
