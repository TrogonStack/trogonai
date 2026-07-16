---
number: "0000"
slug: adr-process
status: accepted
date: 2026-07-16
---

# ADR 0000: Architecture Decision Record Process

## Context

This repository records architecture decisions as ADRs under `docs/adr/`, but the
process that governs them has been implicit. There is no record of what states an
ADR can hold, how one advances from proposal to accepted, or how a decision is
retired or superseded. Without a written process, status values, review
expectations, and numbering drift over time.

The Straw Hat Team publishes a mature, self-contained ADR process that already
defines a lifecycle, review quorum, and editor role:

- <https://straw-hat-team.github.io/adr/adrs/0000000000/README.html>

Adopting an existing, battle-tested process is preferable to inventing local
terms. However, adopting it purely by reference would be brittle: the external
page can move or change, and it carries governance specifics (numbering width,
capitalized status vocabulary, a named editor list) that do not all match this
repository. This ADR adopts the framework and pins the local adaptations so the
process stays stable and self-contained even if the upstream page changes.

## Decision

This repository follows the Straw Hat Team ADR process for lifecycle and review,
with the adaptations recorded below. Where this ADR is silent, the upstream
process is the reference.

### Lifecycle States

The `status` frontmatter is validated by the docs tooling
(`docs/.vitepress/helpers.ts`), which is the authoritative list of persisted
states. An ADR holds exactly one of these, mapped to the upstream semantics:

| `status` | Upstream state | Meaning |
| --- | --- | --- |
| `draft` | Draft | Under discussion and iteration; not yet agreed. |
| `accepted` | Approved | Agreed and treated as current guidance. |
| `rejected` | Rejected | Declined; kept for the record and the rationale. |
| `superseded` | Replaced | Superseded by a later ADR, which it should link to. |
| `deprecated` | (none) | Previously accepted, no longer current guidance, without a single replacement. |

The upstream Reviewing, Withdrawn, and Deferred states are not persisted in
frontmatter. Reviewing is the pull request under review; a proposal that is
withdrawn or deferred is one whose pull request is closed or left unmerged.

### Review and Advancement

- Advancing to `accepted` requires formal signoff from an approver other than the
  author, with no unresolved objections.
- When an approver authors the ADR, another approver must sign off.
- The approvers for this repository are its maintainers, not the upstream editor
  roster. The upstream document's named editors do not govern this repository.

### Local Adaptations

These are the deliberate deltas from the upstream process:

- **Numbering.** ADRs use a four-digit zero-padded number (`0001`, `0023`), not
  the ten-digit upstream form. This ADR is `0000`, reserved for the process
  itself.
- **Status vocabulary.** The `status` frontmatter uses the lowercase tokens above,
  enforced by the docs build. This repository's `accepted` corresponds to the
  upstream `Approved` state, and `superseded` to `Replaced`; the meanings are
  identical.
- **Persisted states only.** This repository models the durable outcome of an ADR
  in frontmatter and leaves transient review flow to the pull request, rather than
  encoding every upstream state as a status value.

### File and Frontmatter Convention

Each ADR is a Markdown file named `NNNN-slug.md` under `docs/adr/`, with
frontmatter carrying `number`, `slug`, `status`, and `date`, followed by a
`# ADR NNNN: Title` heading. The filename must match `number` and `slug`, and the
`status` must be one of the values above; both are checked by the docs build. A
new ADR is added to the list in `docs/adr/index.md`.

## Consequences

- The repository has one written, self-contained process for proposing,
  reviewing, and retiring architecture decisions.
- The lifecycle vocabulary is explicit and aligned with the enforced frontmatter,
  so a decision can be marked `superseded` or `deprecated` rather than only
  `accepted` or `rejected`.
- Existing ADRs remain valid without renaming or status migration: their
  `accepted` and `rejected` values already match the states defined here.
- The process survives changes to the upstream page, because the parts this
  repository depends on are recorded here rather than only linked.

## References

- [Straw Hat Team ADR process](https://straw-hat-team.github.io/adr/adrs/0000000000/README.html)
- [ADR index](./index.md)
