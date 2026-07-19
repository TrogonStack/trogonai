---
term: "Epoch"
section: "WebAssembly execution"
order: 5
---

# Epoch

wasmtime's wall-clock backstop: a background ticker interrupts a guest call that
runs too long even if it has fuel left. See
[Decider Platform](../architecture/decider.md#engine-budgets-and-deadlines).
