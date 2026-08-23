# Session Structured Diff

A review UI wants to highlight changed lines without fetching both versions of
every file. This page documents the artifact that carries that, and why it is an
artifact rather than an event. It documents the protobuf contract that exists
today. There is no Rust implementation yet.

See [Session Artifacts](./session-artifacts.md) for how the bytes are stored and
read.

## It is artifact payload, not schema

`DiffSummary` already had the right shape: exact line counts inline, and the
rendered diff out of line behind a claim-check. What was missing was a typed form
for what sits behind that claim-check, so a reviewer does not have to parse
unified diff text to know which lines changed.

Putting hunks on `FileChanged` was the alternative and it is wrong for a reason
that has nothing to do with size. How much surrounding context to show is a
presentation decision that changes when a review tool changes. Encoded into
events, every such decision becomes permanent history in a log that is never
truncated ([ADR#0035](../adr/0035-session-store-decider-aggregate.md) facet 7).
The domain would be carrying a UI's opinion about context lines forever.

It lives in its own subtree, `sessions/diff/v1alpha1`, and imports nothing from
the write side ([ADR#0035](../adr/0035-session-store-decider-aggregate.md) facet
3). A diff format moves on a review tool's schedule, and nothing in the session
domain should have to move with it.

## The counts are not recomputed from the hunks

This is the trap. Given a structured diff, counting added and removed lines looks
like something a reader can just do, which would make `DiffSummary.added_lines`
redundant.

It is not, because `DiffSummary.truncated` exists. The artifact may omit hunks
while the inline counts stay exact. A reader that recounts from the hunks of a
truncated diff gets a smaller number and is confidently wrong, and the failure is
silent: two fields for one fact, and the one that disagrees is the one the caller
trusted.

The inline counts stay authoritative. The artifact is for showing, not for
counting.

## The marker is the kind, not the first character

`DiffLine.text` holds the line without its leading `+`, `-`, or space, and
`LineKind` says which it was.

A renderer that strips one character off the front of a line and assumes it was
punctuation will eventually strip a character that was content. Making the marker
a typed field means that never comes up.

`old_line` and `new_line` are absent rather than zero when the line does not
exist on that side, so "this line did not exist before" cannot be read as "line
zero".

## Elision is a line, not a gap

`LINE_KIND_ELIDED` stands for a run of omitted lines and carries `elided_count`
instead of text.

Without it, a truncated diff renders two distant regions as adjacent code, which
is not a missing detail but an actively misleading picture of the file. A reader
looking at the result cannot tell that anything was left out.

## It says what it is, on its own

`format_version` and `truncated` are both on the artifact, duplicating what the
claim-check and `DiffSummary` already imply.

That is deliberate. An artifact outlives the reference that found it. A stored
diff read years later through a generic artifact path has to be able to say what
it is and whether it is complete without the `mime` string and the `DiffSummary`
that pointed at it. `format_version` is incremented when the meaning of an
existing field changes, not when fields are added.

## Layout

`proto/trogonai/session/sessions/diff/v1alpha1/structured_diff.proto`

| Type | What it settles |
| --- | --- |
| `StructuredDiff` | the parsed diff, self-describing and versioned |
| `DiffHunk` | one changed region with its context |
| `DiffLine` | one line, with its side positions |
| `LineKind` | what the line represents, including what is not there |

Discriminated by `ArtifactRef.mime`, which is required on every `ArtifactRef` and
is the media-type mechanism the artifact contract already uses.

## Status

Shipped: `structured_diff.proto`, lint-clean, formatted, building, and generating
Rust bindings reachable at `trogonai_proto::session::sessions::diff_v1alpha1`.

Not shipped: the differ that produces it and the media type string it is
published under.
