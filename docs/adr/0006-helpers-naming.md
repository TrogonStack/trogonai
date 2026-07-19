---
number: "0006"
slug: helpers-naming
status: accepted
date: 2026-06-08
---

# ADR#0006: Helpers Naming

## Context

Straw Hat has an approved organization-wide [ADR](../glossary/adr) for the long-running `Helper`
versus `Util` naming debate. TrogonAI should not reopen that choice locally or
let language-specific habits create inconsistent names across the monorepo.

This repository also prefers domain-specific names over vague buckets. That
preference remains stronger than choosing between generic words.

When a generic container is still necessary, the remaining local decision is the
single file or directory name to use for that container.

## Decision

Respect and follow
[Straw Hat ADR 1146361044: Helper vs Util](https://straw-hat-team.github.io/adr/adrs/1146361044/README.html).

When one generic support-code container file or directory is unavoidable, name
it `helpers`.

Do not use `helper`, `util`, `utils`, or `utilities` for generic support-code
containers.

Prefer a domain, capability, or role name when one is available. Do not use this
decision to justify vague package, module, file, type, or function names.

## Consequences

- Code review should reject new generic support-code containers named `helper`,
  `util`, `utils`, or `utilities`.
- Existing names can be corrected when touched for related work.
- Repository-specific ADRs about package boundaries and domain names still
  apply before falling back to a generic `helpers` container.

## References

- [Straw Hat ADR 1146361044: Helper vs Util](https://straw-hat-team.github.io/adr/adrs/1146361044/README.html)
- [ADR#0002: Rust Crate Boundaries](./0002-rust-crate-boundaries.md)
