# Session Tool Effects

A tool call is not one effect. A search touches thousands of filenames and reads
none of them. A copy creates a file that is not new content. A batch edit over
ten targets succeeds for eight. This page documents three additions that let a
completion say what actually happened, and the volume constraint that shapes all
three. It documents the protobuf contract that exists today. There is no Rust
implementation yet.

See [Session Aggregate](./session-aggregate.md) for `ToolCallCompleted` and
`FileChanged`, and [Session Artifacts](./session-artifacts.md) for the
claim-checks these lean on.

## The constraint that shapes all of it

`ResourceObservation` already carries the rule, in its own header: record an
observation only for a resource whose content actually entered the model's
context, never for every path a search walked. Reads outnumber writes by more
than an order of magnitude, and the log is never truncated
([ADR#0035](../adr/0035-session-store-decider-aggregate.md) facet 7).

Everything below obeys that. None of these additions is per path.

## Namespace access is not content access

A grep across a secrets directory that returns no matches reads nothing. It
produces no `ResourceObservation`, correctly. And yet something happened that a
compliance reviewer has to be able to see: the agent learned those files exist
and what they are called.

The tempting fix is to weaken `ResourceObservation` so it can also mean "saw the
name". That destroys both questions. "Did the agent read this file" and "did the
agent see this file's name" have different answers and different consequences,
and a record that means either answers neither.

`ResourceAccessRecord` is the separate fact. It is per scope, never per path:

- `scope` is the extent of exposure, a directory or a search root
- `matched` is what came back, which is the number that matters, because four
  hits for "password" in a secrets directory is a different disclosure from four
  hundred files existing there
- `traversed` is what was walked to produce that answer, counted separately
  because reading a filename to reject it is still reading the filename
- `complete` says whether the scope was covered, so a truncated enumeration is
  never read as a full inventory

The path list itself, when it is worth keeping, goes out of line in `enumerated`
as a claim-check. That is the volume argument again, and it has a second effect
worth naming: the list is erasable under a deletion request while the counts
survive it. The audit fact that an enumeration happened stays permanent even
after what was enumerated is gone.

`ResourceAction` separates the actions that expose names from the ones that
expose or alter content, because a real policy is written that way. "This agent
may find files here but may not open them" is a rule deployments have, and it is
inexpressible without `LIST` and `SEARCH` being distinct from `READ`.

## A copy is not a create, and not a rename either

`FileChanged` had four kinds, and a copy had to be recorded as a create. That
loses the fact that makes it reviewable. "A new file appeared at
`deploy/prod.yaml`" and "`deploy/staging.yaml` was duplicated to
`deploy/prod.yaml`" are different events, and the second raises a question worth
raising.

The available shortcut was to reuse `previous_path`, since both a rename and a
copy name an earlier location. They make opposite claims about it. After a rename
the previous path is gone; after a copy the source is still sitting there,
unchanged. Overloading the field would report a file as moved when it was not, so
`previous_path` stays rename-only and `CopySource` is its own field.

`FILE_CHANGE_KIND_COPIED` replaces `CREATED` for the destination rather than
appearing alongside it, so the destination has exactly one kind and two facts
about one change cannot disagree. The source emits nothing, because nothing
happened to it.

`CopySource.source_digest` is what makes the copy checkable later, on the same
reasoning as `ResourceObservation`: without it, a reader can see that
`prod.yaml` came from `staging.yaml` but not whether it came from the
`staging.yaml` that exists now or from a version since edited.

## Eight of ten is a completion, not a failure

A batch edit over ten files that applies eight and fails two is a
`ToolCallCompleted` with `TOOL_CALL_RESULT_STATUS_APPLICATION_ERROR`. It is never
a `ToolCallFailed`.

The reason is not classification taste. Eight files really changed. Those changes
are already `FileChanged` facts on the log and no result status retracts them. A
`ToolCallFailed` asserts that nothing was applied, and a reader that folds it
that way will believe a workspace is in a state it is not in.

`failed_targets` records only the failures. The successes are deliberately
absent, because they exist as `FileChanged` and a second record of the same
change is a second thing to keep consistent through rewind and redaction. What no
other event can express is the negative: that a target was named, attempted, and
did not happen. Without it, a reader seeing eight changes for a ten-file request
has to reconstruct the gap from tool arguments, which is exactly the inference a
typed record removes.

`targets_attempted` is the denominator. It is recorded rather than inferred,
because zero targets attempted and no target list are different situations that
an empty list cannot tell apart.

`TargetFailureReason` is typed because retry soundness differs by reason. A
permission fault is durable and a retry repeats it. A failed precondition means
the resource moved under the agent, and a retry against fresh content may
succeed. `NOT_ATTEMPTED` is neither: it says the call stopped before reaching
this target, which is not a fact about the target at all, and it exists so an
unreached target is never read as a rejected one.

`target_uri` is a URI rather than a workspace-relative path because a
multi-resource tool's targets are not always files, and a failure against a
remote resource has no path to record.

## Never a failed FileChanged

The rule these share: `FileChanged` means a file changed. A target that failed
did not change, so it produces no `FileChanged` under any circumstance, no matter
how convenient a `success: false` flag would be for a renderer. The moment that
flag exists, every consumer of file history has to remember to filter on it, and
the ones that forget will report changes that never happened.

## Layout

| File | What it settles |
| --- | --- |
| `v1alpha1/resource_access.proto` | namespace access as distinct from content access |
| `v1alpha1/copy_source.proto` | where copied content came from, and what it hashed to |
| `v1alpha1/target_outcome.proto` | which targets of a multi-target call did not apply |

They attach at `ToolCallCompleted.accessed`, `ToolCallCompleted.failed_targets`
with `targets_attempted`, and `FileChanged.copied_from`, with the matching fields
on `CompleteToolCall`.

## Status

Shipped: all three protos plus `FILE_CHANGE_KIND_COPIED`, lint-clean, formatted,
building, and generating Rust bindings reachable at
`trogonai_proto::session::sessions::v1alpha1`.

Not shipped: any tool that populates them. A tool runtime has to decide what its
scope is and count its own traversal, and nothing does that yet.
