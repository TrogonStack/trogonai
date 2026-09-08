# Session Terminal Replay

A long command interleaves stdout and stderr across millions of writes. The
session log records a claim-check to the captured output and never the output
itself. This page documents the shape of that capture, what a reader is allowed
to conclude from it, and where it attaches on the write side. It documents the
protobuf contract that exists today. There is no Rust implementation yet.

See [Session Artifacts](./session-artifacts.md) for how the bytes are read and
checked, and [Session Aggregate](./session-aggregate.md) for the events involved.

## Why this is an artifact and not events

An event per output chunk would put the size of the log at the mercy of the
noisiest command that ever ran on it, permanently, because nothing ever leaves
the log ([ADR#0035](../adr/0035-session-store-decider-aggregate.md) facet 7).
Every reader that folds the session would pay for it forever, including readers
that never render a terminal.

So the capture is one immutable, content-addressed blob referenced by
`CommandOutputReplayRef`, and the whole of item 9's machinery applies to it:
bounded range reads, chunk-hash verification, and the availability vocabulary
for when the bytes are gone.

## It attaches to the execution record, not the transcript

`CommandOutputReplayRef` sits on `ToolCallCompleted` beside `CommandTermination`,
and deliberately not inside `ToolCallResult`.

The reason is the one already written on `CommandTermination`: `ToolCallCompleted`
owns the execution and audit fold, while `ToolCallResult` owns the
provider-visible transcript the model actually received. Those two routinely
differ, because what reaches the model is truncated, summarized, or both. Raw
captured output placed in the replay shape would hand a later turn context that
the original turn never had, which is the same failure an exit code in the
transcript would be.

It goes on the completion and not the failure for the reason `termination` does.
A command killed by a timeout ran, so it completes with a signal termination and
has output; a command that never started is a `ToolCallFailed` and has none.

`CompleteToolCall` carries the same field, and the capture is expected to be
sealed and stored before the command is issued. A decider that had to write 40 MB
before it could append would be one whose append latency is set by the noisiest
command in the system.

## Frames are a fact about the reader loop

`ReplayFrame` is one contiguous run of bytes the capturer read in one go. It is
not a line, not a `write()` by the process, and not a flush.

This matters because a renderer that treats frames as lines will split a line
that happened to arrive in two reads, and it will split it in a different place
on a fast machine than on a slow one. The framing exists to preserve order and
stream attribution, and nothing else should be read out of it.

Payload bytes are stored raw. Not decoded, not normalized, and specifically not
stripped of control sequences, because a terminal replay with the escape codes
removed is replaying something the terminal never showed. Decoding is the
renderer's decision, and it gets to make it more than once.

## Capture mode decides what the ordering is worth

This is the field most likely to be ignored and the one most likely to make a
UI lie.

`CAPTURE_MODE_SEPARATE_PIPES` means stdout and stderr were read from two pipes.
Stream attribution is exact. The interleaving is not: the child's own buffering
decides when each pipe becomes readable, so a line written to stderr before a
line written to stdout can easily be captured after it. A renderer may color the
two streams and must not present their relative order as the order the process
produced them in.

`CAPTURE_MODE_MERGED_TERMINAL` means both went to one terminal or one pipe. The
interleaving is the process's own and is exact. Stream attribution is gone, and
every frame says `REPLAY_STREAM_MERGED`, because by the time the capturer saw
the bytes there was nothing left to distinguish. A renderer must not infer
stderr from content to fill the gap.

Neither mode is the better one. They lose opposite things, the choice is made at
spawn time, and a renderer cannot see that choice unless the capture records it.
Which is why it is a required field rather than a default.

## Order is not timing

`TIMING_FIDELITY_ORDER_ONLY` is the common case, and it is stated rather than
inferred because a replay UI that animates untimed frames is inventing pacing and
presenting it as a recording.

`TIMING_FIDELITY_CAPTURE_ELAPSED` means frames carry an elapsed duration, and the
name is deliberate: it is when the *capturer* observed the frame, not when the
process wrote it. Pipe buffering, scheduler delay, and the capturer's own read
loop all sit in between. It is close enough to play back at speed and it is not
evidence of when anything happened.

## Which end went missing

`ReplayCompleteness` is not `ArtifactRef.truncated`, which says a preview is
shorter than its content. This says the content is shorter than what the process
emitted, and it names which end is gone, because for a command log that is the
entire question. Losing the tail loses the failure. Losing the head loses what
was run.

`TRUNCATION_SHAPE_UNSPECIFIED` is not complete. A reader that meets a shape it
does not recognize must treat the capture as of unknown completeness rather than
whole.

`dropped_byte_count` and `dropped_frame_count` are optional, and absent means
dropping occurred and was not counted, which a fixed-size ring buffer that
overwrites without accounting cannot avoid. They are left unset rather than
reported as zero, because zero here would say nothing was lost.

Frame sequence numbers are the capturer's original count and are never compacted
when frames are dropped, so a `MIDDLE_DROPPED` gap is visible in the frames
themselves. Renumbering to stay contiguous would destroy the only evidence
inside the artifact that anything is missing. The header field is what lets a
reader know to expect a gap without scanning for one.

## The index is what makes a range read usable

Range reads are byte-granular. Frames are variable-length. A read into the middle
of the frame region lands mid-frame, with nothing in the bytes to say where the
next boundary is, so without an index the only correct way to reach frame one
million is to parse the previous 999,999. That is the cost keeping the output out
of the log was supposed to avoid, reintroduced one layer down.

`ReplayIndex` is a sparse trailer. Sparse, because an entry per frame is an index
whose size grows with the output it indexes, which is the original problem with
more steps; a reader seeks to the nearest preceding entry and scans forward a
bounded distance. A trailer, because a capturer streaming a running process does
not know the index until the process exits and so cannot write it as a header.

Its offset and length are recorded on the log, so the first read is a bounded
range read straight at the index, rather than a seek relative to an
end-of-object the artifact store might report differently than the log does.

`ReplayIndexEntry` carries three offsets that are not redundant, because they
answer different questions and converting between them means parsing the frames:

- `byte_offset` is what a range read takes.
- `frame_sequence` is what a gap shows up in, which is why it is recorded rather
  than computed as entry position times a stride.
- `output_byte_offset` is what a UI scrolls and measures in, since a user
  positions themselves in output and not in framing overhead.

`elapsed` is the fourth, present only for a timed capture, and it is what makes
seeking by time possible without reading anything.

`ReplayIndex` repeats `frame_count` and `output_byte_count`, which are also on
the event. That repetition is the point, and it is the same rule as the artifact
chunk manifest: a reader that has fetched only this trailer can check it against
the log before trusting a single offset in it, and the log is the copy the
artifact store did not supply. An index that disagrees with the event is not a
stale index, it is the wrong artifact or a damaged one.

## What survives erasure

Every fact a reader needs to describe the capture is on the log rather than only
inside the artifact: capture mode, timing fidelity, completeness, frame count,
output byte count.

So when the bytes are erased ([Session Artifacts](./session-artifacts.md)), the
session can still say that the command produced 2.1 million frames and 40 MB,
was captured through a terminal, and was complete. That is the whole point of
separating artifact byte lifecycle from log retention, and it only works if the
summary was never stored exclusively in the thing that gets deleted.

## Layout

Two homes, split the way `content_chunks.proto` and the `artifacts` subtree are
([ADR#0035](../adr/0035-session-store-decider-aggregate.md) facet 3).

The write side records where the capture is and what it is worth:

- `trogonai/session/sessions/v1alpha1/command_output_replay.proto`:
  `CommandOutputReplayRef`, `CaptureMode`, `TimingFidelity`,
  `ReplayCompleteness`, `TruncationShape`.
- `output_replay` on `ToolCallCompleted` and `CompleteToolCall`, field 9.

The read side describes the bytes inside it:

- `trogonai/session/sessions/replay/v1alpha1/frame.proto`: `ReplayFrame`,
  `ReplayStream`, and the byte layout of format version 1.
- `trogonai/session/sessions/replay/v1alpha1/index.proto`: `ReplayIndex`,
  `ReplayIndexEntry`.

The artifact is two regions. The frame region runs from offset 0 and holds
frames back to back, each a little-endian `uint32` byte length followed by that
many bytes of a serialized `ReplayFrame`. The index region is the trailer,
holding a serialized `ReplayIndex`, located by `index_offset` and `index_length`
on the event.

## Status

Shipped: the three protos above and the two field additions, lint-clean,
formatted, building, and generating Rust bindings reachable at
`trogonai_proto::session::sessions::replay_v1alpha1`.

Not shipped: the capturer, the spool-and-seal path, the framing writer, the
index builder, the renderer, and the replay-artifact byte quota, which belongs
with the rest of the artifact admission limits rather than here.

`format_version` is on the event as a number rather than parsed out of the
artifact's media type, because a reader deciding whether it can parse these
frames should not be doing string surgery on a MIME parameter to find out. There
is exactly one version and the field exists anyway, since the alternative is
adding it after the first capture that needs it, when every existing artifact
already lacks it.
