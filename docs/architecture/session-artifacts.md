# Session Artifacts

A command produces 40 MB of output. The session log records a claim-check to it
and never the bytes. This page documents how a caller gets part of those bytes
without pulling all of them, how it establishes that what it got is what the log
promised, and what it is told when the bytes are not there. It documents the
protobuf contract that exists today. There is no Rust implementation yet.

See [Session Aggregate](./session-aggregate.md) for how artifacts are recorded,
[Session Doctor](./session-doctor.md) for the check that probes the artifact
store, [Session Query Contract](./session-queries.md) for the read model this
extends, and [Session Terminal Replay](./session-terminal-replay.md) for the
artifact class that leans hardest on range reads.

## A whole-artifact digest cannot check part of an artifact

`ArtifactRef.digest` covers every byte. It is the claim-check key and the
content-addressing key, and checking anything against it means hashing 40 MB.
That is the exact cost a range read exists to avoid, so a caller fetching 64 KB
has two options: pay the full cost anyway, or skip the check while still holding
a digest that looks like it proved something. The second is what actually
happens.

So an artifact is also hashed in fixed-size chunks, and the digest of that
ordered chunk-digest list is recorded on the log by `ArtifactRecorded`, in
`StoredArtifact.chunks`. Verification then has three steps, and the order of
them is the whole design:

1. The caller reads `ContentChunks.manifest_digest` from the session's own
   history. This value came from the event log.
2. The caller fetches the chunk manifest, hashes it, and compares. This is the
   step that matters, because the manifest comes from the artifact store and a
   store serving wrong bytes will happily serve a manifest that agrees with
   them. Compared against the log, it cannot.
3. The caller hashes the chunks it received and compares them to the manifest it
   just established. Cost is proportional to what it read, not to the artifact.

Trust flows outward from the log. A manifest checked only against the store that
served it proves that the store is self-consistent, which is a property a
corrupted store also has.

The chunk count is deliberately not recorded next to the chunk size: it follows
from `StoredArtifact.size_bytes`, and a second field carrying a derivable fact
is a second field that can disagree with the first.

## The server does not verify, and does not claim to

`ReadArtifactRangeResponse` reports `RangeVerifiability`, not a verification
result. There is no field on it meaning "verified".

A server field saying "verified" would be the party under suspicion certifying
itself. The reason to check an artifact at all is that the store might return
the wrong bytes, and a store that returns the wrong bytes will also set a
boolean to true. The only check worth anything is one the caller performs
against a value the store did not supply.

That fixes this contract's obligation, and it is a narrower one than it looks:
serve ranges in a shape the caller can check, and say plainly when it has not.
`RANGE_VERIFIABILITY_VERIFIABLE` means the served range covers whole chunks, so
every byte in it falls under a chunk digest. `NO_MANIFEST` means the artifact
was stored without chunk hashing and nothing short of a full read can check it.
`RANGE_NOT_ALIGNED` means a manifest exists and the caller asked for bytes that
do not line up with it, so the partial chunks at the edges are uncovered.

Verified reads are therefore chunk-granular. A request for bytes 100 through 200
of a 1 MB-chunked artifact is served as the whole first chunk, and the response
says so. Which is why the response reports the offset actually served rather
than echoing the request: a caller that assumes it received what it asked for
will misread the buffer by 100 bytes, and it will misread it silently.

## Requiring verification means refusing, not warning

`VerificationRequirement` defaults to required when unset.

The default has to be the strict one. A caller that never thought about
verification is exactly the caller who will not notice unchecked bytes arriving,
and a field whose omission quietly weakens a guarantee is a guarantee only for
people who read the schema.

And when the requirement cannot be met, the read fails with
`ARTIFACT_ERROR_CODE_VERIFICATION_UNAVAILABLE` rather than returning bytes
labelled unverifiable. A requirement that degrades into a label when it cannot
be satisfied is not a requirement. The caller asked not to receive unchecked
bytes; handing them over with a note attached is delivering the thing they asked
to avoid, and doing it in a field they have no reason to read.

`NOT_REQUIRED` is a real option and belongs to a preview pane. It does not
belong to anything that will be executed, replayed, or stored elsewhere as the
artifact.

## The response says where it is, so nothing has to say whether it is done

`ReadArtifactRangeResponse` carries the served offset, the served bytes, and the
artifact's total size. It carries no `truncated` or `eof` flag, and no separate
length field.

Length is the length of `content`; a second copy of it is a value that can
disagree with the bytes next to it, and the copy is the one a caller would
trust. End-of-artifact follows from `offset + len(content) == size_bytes`; a
flag would be a second answer to a question the numbers already settle. Being
cut short by a server cap is the same comparison coming out the other way. A
caller resuming a sequential read knows where to resume without asking again.

This is the same reason `WriteOutcome` derives disposition from state rather
than carrying it: two fields for one fact can disagree, and the one that
disagrees is the one a caller trusted.

## Statting an artifact cannot fetch it

`StatArtifactRequest` has a session id and an artifact id. It has no range, no
"include content", and no size threshold under which the bytes come along.

The structural argument is the one that gives `DiagnoseSession` no repair mode.
A caller asking what a 40 MB artifact is must not be able to fetch 40 MB by
setting a field wrong, and a reviewer must not have to read a runtime value to
know a call was cheap.

`StatArtifactResponse` also carries the recorded preview, which is on the log
rather than in the artifact store. So it survives the bytes: an erased artifact
still previews. For a great many callers the preview is the whole answer, and
the reason it is not modelled as a range read is that it costs nothing to serve.

`ReadableContent` is set only for an available artifact. A size and digest
reported next to `ERASED` would be describing something the caller cannot
obtain.

## Absence has kinds, and they are not interchangeable

`ArtifactAvailability` does not have a "missing" value that covers everything
that is not the bytes. Collapsing them is how deliberate destruction gets
reported as data loss.

- `ERASED`: destroyed on purpose and recorded by `ArtifactErased`. The
  claim-check and its provenance stay on the log. This is a correct end state,
  and a reader that renders it as damage sends someone looking for a problem
  that was the point.
- `MISSING`: the store has no object and no erasure was ever recorded. This is
  the one value that means something went wrong. The log says bytes exist that
  do not.
- `EXTERNAL_ONLY`: recorded as an external reference whose bytes were never
  durably stored. Nothing was lost; nothing was ever held.
- `CORRUPT`: read, and does not match the recorded digest. Serving these bytes
  quietly would hand a caller content under a claim-check it does not satisfy.
- `UNREADABLE`: could not be read, which makes no claim about whether the bytes
  exist. A permissions or transport fault against the artifact store looks
  identical from here and a retry may succeed. Reported as `MISSING` it sends an
  operator after a backup instead of a broken credential.
- `HIDDEN`: the caller may see that the artifact exists and may not read it.

The first five reuse the doctor's existing mismatch, absent, and unreadable
distinction rather than inventing a second vocabulary for the same three facts.

`HIDDEN` is the one that needs defending, because elsewhere in this system
authorization hides existence: `QUERY_ERROR_CODE_SESSION_NOT_FOUND` deliberately
does not distinguish a missing session from a forbidden one, so that probing
cannot use the difference to prove a session exists. The difference here is that
by the time an artifact id can be named, the caller is already reading a session
whose history contains the claim-check. The reference is visible; only the
content is withheld. Answering `MISSING` would not conceal anything, it would
just describe the session incorrectly, and it would say bytes were lost when
they were not. An artifact inside a session the caller cannot see is never
reachable to be labelled at all.

Every one of these rides on a successful response. `ArtifactError` is for
failures to answer, so that "what happened to this artifact" stays
distinguishable from "the read contract is broken" at the moment an operator
needs to tell them apart. The one pair worth reading twice:
`ARTIFACT_ERROR_CODE_ARTIFACT_NOT_FOUND` means the session holds no claim-check
under that id, so nothing was ever promised; `ARTIFACT_AVAILABILITY_MISSING`
means the log made a promise the store did not keep.

## Successive ranges are the streaming primitive

There are no `service` definitions anywhere in `proto/`. Transport is JSON-RPC
over NATS ([ADR#0055](../adr/0055-nats-subject-design-jsonrpc-bindings.md),
[ADR#0056](../adr/0056-canonical-jsonrpc-bodies-over-nats.md)) and the protos are body
types, so there is no streaming call to bind to. A caller reading 40 MB issues
successive bounded ranges.

Stating this is better than inventing a streaming RPC that nothing implements.
It also has a property worth keeping: the bound is always the caller's and
always visible in the request, rather than a limit discovered when a response
fails to fit. A server cap is still possible and is reported through
`ARTIFACT_ERROR_CODE_RANGE_TOO_LARGE`, whose accepted maximum is carried in the
message because it is deployment-specific rather than a schema constant.

## Completeness is not lifecycle

`SessionView.artifacts` reports how much of a session's artifact content is
still retrievable, separately from whether the session succeeded.

The two are independent and routinely disagree. A session can close having done
exactly what was asked and still be missing the output it produced, because
artifact bytes live outside the log: they get erased under a retention policy,
under a deletion request, or by a storage fault. A reader that infers
retrievability from `TERMINAL_REASON_CLOSED` will present an empty result as a
successful one.

`ArtifactCompletenessView` counts only what the log itself establishes:
recorded, claimed retrievable, erased, external-only, and hidden from this
caller. Whether the bytes behind an intact claim-check are actually present is a
fact about an external store, and learning it costs one probe per artifact,
which is not something a query can do and remain a query.

That answer arrives through `observed`, set only when someone has actually
looked. Unset means nobody has, which is not the same as nothing being wrong,
and the field is absent rather than zeroed precisely so the two cannot be
confused. `ObservedIntegrityView` carries `missing`, `digest_mismatch`, and
`unreadable`, and an `observed_at`, because these counts describe a store at an
instant and an artifact can be lost the moment after. A stale all-clear is not a
current one.

`hidden` is per-caller, so two readers of the same session can see different
totals. Surfacing it is the point: the difference is authorization, not damage.

The rollup is on `SessionView` and not on `SessionSummary`. Unlike
`RecoveryProvenanceView`, which is on the list row because a picker rendering a
salvaged session identically to an intact one is where a substitution actually
happens, an incomplete artifact view does not change what the session is. It
changes what can be retrieved from it, which is a thing you find out when you
open it.

## Layout

The read contract lives in its own sibling subtree,
`trogonai/session/sessions/artifacts/v1alpha1`, alongside `doctor` and
`maintenance`, and redefines its value types locally rather than importing them
from the write side ([ADR#0035](../adr/0035-session-store-decider-aggregate.md)
facet 3). The write side records what happened to an artifact; this contract
answers what a caller can get right now, and the two change for different
reasons on different cadences.

- `availability.proto`: `ArtifactAvailability`.
- `stat_artifact.proto`: `StatArtifactRequest`, `StatArtifactResponse`,
  `ReadableContent`.
- `chunk_manifest.proto`: `GetChunkManifestRequest`,
  `GetChunkManifestResponse`.
- `read_artifact_range.proto`: `ReadArtifactRangeRequest`,
  `VerificationRequirement`, `ReadArtifactRangeResponse`, `RangeVerifiability`.
- `artifact_error.proto`: `ArtifactError`, `ArtifactErrorCode`.

The durable half is one message on the write side,
`trogonai/session/sessions/v1alpha1/content_chunks.proto`, held there because it
is recorded by `ArtifactRecorded` on `StoredArtifact.chunks` and a sibling
subtree does not get to define fields on a write-side event.

A typed, versioned diff is one of the payloads that sits behind a claim-check
rather than in the schema; see
[Session Structured Diff](./session-structured-diff.md).

## Status

Shipped: the six protos above and the `SessionView` rollup, lint-clean,
formatted, building, and generating Rust bindings reachable at
`trogonai_proto::session::sessions::artifacts_v1alpha1`.

Not shipped: the artifact store binding, chunk hashing at record time, the range
reader, the availability resolver, the completeness projection, the doctor probe
that fills `observed`, and the transport binding.

`StoredArtifact.chunks` is optional, and it stays optional. Artifacts recorded
before chunk hashing existed do not have one and cannot retroactively acquire
one from a log that does not contain their bytes, and a small artifact does not
need one. Absence is a first-class answer here rather than a migration to
finish: it means no range of that artifact can be checked, the read contract
says exactly that, and a caller that required verification is refused rather
than quietly served.
