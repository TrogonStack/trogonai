---
number: "0014"
slug: command-and-query-naming
status: accepted
date: 2026-06-30
---

# ADR#0014: Command and Query Naming

## Context

The scheduler [crate](../glossary/crate) organizes its write side as commands and its read side as
queries. The command modules already read as verb + noun
(`create_schedule`, `pause_schedule`, `resume_schedule`, `remove_schedule`,
`record_schedule_occurrence`, `schedule_next_occurrence`), so the file name
states the operation and the entity it acts on.

The query modules drifted from that shape. They were named by the bare verb
(`queries/get.rs`, `queries/list.rs`, and the same under `queries/projection/`),
while the functions they contained were already verb + noun
(`get_schedule`, `list_schedules`). The module name and the function name
disagreed, and a reader scanning the directory saw `get` and `list` with no
indication of what they operate on.

Bare verbs do not scale. A second entity in the same module forces either a
rename or a second `get` that collides in meaning, and the directory stops
communicating the domain at a glance.

## Decision

Commands and queries must be named verb + noun.

- The noun is the domain entity the operation acts on, not a generic word.
- The module or file name must match the public operation it exposes
  (`get_schedule.rs` exposes `get_schedule`).
- This applies to every backend of the same query. A query that reads from
  more than one projection uses the same verb + noun name under each backend
  directory.

Do not name a command or query by the bare verb (`get`, `list`, `create`,
`remove`). Do not let a module name and the operation it exposes disagree.

## Consequences

- Code review should reject new commands or queries named by a bare verb, and
  reject a module whose name does not match the operation it exposes.
- The scheduler query modules are renamed to match: `queries/get.rs` and
  `queries/list.rs` become `queries/get_schedule.rs` and
  `queries/list_schedules.rs`, with the same rename under `queries/projection/`,
  and the `mod` / `pub use` lines updated accordingly.
- Existing bare-verb names can be corrected when touched for related work.

## References

- [ADR#0006: Helpers Naming](./0006-helpers-naming.md)
- [ADR#0002: Rust Crate Boundaries](./0002-rust-crate-boundaries.md)
