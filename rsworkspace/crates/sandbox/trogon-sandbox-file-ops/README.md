# Sandbox file operations

`trogon-sandbox-file-ops` executes the contract's `FileOp` requests against a
directory on the host, and holds the value types that file tracking is built on.

## Serving file operations

`FsRoot` is rooted at one host directory. `execute` runs a `FileOp` and always
returns a `FileOpResponse`; `try_execute` returns a typed `FileOpFailure` instead
of folding the failure into the response. `manifest` fingerprints a path or tree.
The root may carry a `SandboxPrefix`, the absolute path the sandbox sees that
directory under, so requests written in sandbox paths resolve to host paths and
paths outside the prefix are refused.

Every path is resolved inside the root. `..` that climbs above it, NUL bytes, and
symlinks that lead outside are refused. On Unix the operations that create, remove,
rename, or open for writing work relative to verified directory descriptors, and
an opened file is checked to still lie inside the root, so a process in the
sandbox cannot swap a directory for a symlink between the check and the open.

Whole-file writes and edits go through a temporary file and a rename, keep the
mode and owner of the file they replace, and run one at a time. Reads, ranged
writes, listing, stat, search, archive export and import, removal, and rename are
served with the limits the contract defines. The tracking verbs
(checkpoint, watch, and the rest) are answered `Unsupported` here because they
need a manifest source that outlives one request.

## Value types

| Type | Purpose |
| --- | --- |
| `FileOpFailure` | Why an operation produced no result; converts to a `FileOpResponse`. |
| `SandboxPrefix` | The validated absolute prefix the sandbox sees the root under. |
| `GlobPattern` | A compiled path glob where `*` stays within a segment. |
| `EditSpec` | A validated find, replace, and expected-occurrence count. |
| `EditOutcome` | What an edit changed, or why it did not apply. |
| `ArchiveSummary` | A tar stream vetted to hold only plain files and directories that fit the limits. |
| `Manifest`, `ManifestEntry`, `ManifestDiff` | A path-keyed tree fingerprint and the difference between two. |
| `StreamHasher`, `content_hash` | SHA-256 fingerprints written the way the contract spells them. |

## Tracking

`Tracking` is the state machine behind checkpoint, watch, and lease-start
baselines. It diffs successive manifests from a `ManifestSource`, parks and wakes
polls, buffers events with an overflow marker, and expires idle watches. A
`ChangeHint` lets the source say where a change happened so only that subtree is
rescanned, and `WatchOverflow` records why a watch lost events. Implement
`ManifestSource` over whatever can list the tree (the host directory through
`FsRoot`, or a guest) and drive `Tracking` from the worker.

## Where it fits

The request, response, and error vocabulary comes from `trogon-sandbox-contract`;
the subject and wire helpers come from `trogon-sandbox-protocol`. This crate adds
the host-side execution and tracking logic on top and has no network dependency.
