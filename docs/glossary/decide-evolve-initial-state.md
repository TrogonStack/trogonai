---
term: "decide / evolve / initial_state"
section: "Event sourcing and the decider"
order: 2
---

# decide / evolve / initial_state

The decider cycle. `initial_state` seeds state with no events applied; `evolve`
folds one stored event into state and is the only place state changes; `decide`
evaluates a command against current state and returns a decision without mutating
anything. See [Decider Platform](../architecture/decider.md).
