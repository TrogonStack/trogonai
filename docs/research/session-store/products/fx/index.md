# fx (Vercel): how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Mapped field-by-field onto our own catalog in
[fx compared to our session event catalog](./vs-session-events.md).
Evidence snapshot retrieved 2026-08-01. fx ships as a closed-source native
binary, so there is no repository or commit to cite and no source quotes are
possible. Version-sensitive claims were checked against these anchors:

- `fx` v0.3.64, macOS build, `sha256
  7799f2c431160de8eef58f5f9cd06a3f9e340c6998c919735afbc902804d826c`
  (8.5 MiB, single static binary).
- Nine session directories written by that same build under the local
  session root (`$FX_HOME/sessions/`, default `~/.fx/sessions/`), read
  field-by-field.
- The JSON emission string table extracted from the shipped binary. fx
  serializes with hand-rolled literal fragments (`,"tool_call_id":`,
  `{"kind":"compacted_summary","summary":`, and so on), so the binary
  contains the wire spelling of every field and every tagged variant
  verbatim, including paths no local session exercised.
- The binary's own machine-readable surfaces: `fx session <id> --json`,
  `fx sessions --json`, `fx doctor`, `fx --help`.

> Evidence tags. Because this is a black-box reconstruction rather than a
> source read, every structural claim below carries one of two tags:
>
> - **[observed]**: read directly out of session files written by this
>   build. Field names, types, key order, and enum values are facts.
> - **[literal]**: the exact JSON fragment exists in the binary's emission
>   table, but no local session exercised the path. The field's spelling and
>   its membership in the named record are certain; ordering and optionality
>   are inferred from the adjacent literals and are marked where they
>   matter.
>
> Anything neither observed nor present as a literal is under **Open
> questions**, not asserted.

## The storage model

The durable session is a **directory containing an append-only event log,
plus a rebuildable folded-state checkpoint and a set of sidecars**. fx names
the format itself in every manifest it writes: `"storage_format":
"event_log_v1"` [observed].

Session directory layout [observed]:

| Path | Role |
| --- | --- |
| `events.jsonl` | The append-only event log. Source of truth. |
| `checkpoint.json` | Full folded state at a sequence number. Derived. |
| `session.json` | Manifest: identity, counters, log fingerprint, checkpoint pointer. Derived. |
| `authority.json` | Which process/format owns the session directory. |
| `commit.<log_generation>.json` | The durable commit boundary (`through_seq`, `through_event_id`, `through_event_log_bytes`). |
| `commit.lock`, `session.lock` | Advisory locks (zero-byte). |
| `display.json` | Listing sidecar: title, preview, origin workspace root. Derived. |
| `usage-v2.json` | Token/cost accounting snapshot. Derived. |
| `tool-results/result-<tool>-<hash>-<hash>.txt` | Spilled tool output bodies referenced by handle. |
| `artifacts/web-fetch/` | Fetched-content artifacts. |
| `logs/commands/fx-command-replay-<pid>-<nanos>-<idx>-<hash>.bin` | Command output replay tapes, magic `FXRPLY01`. |
| `subagent/relationship-index.bin` | Binary index of subagent relationships (+ `relationship-page-*` pages). |

Store-root files, outside any one session [observed]:

| Path | Role |
| --- | --- |
| `sessions/latest/<sha256(workspace_root)>.json` | Per-workspace "latest session" pointer cache. |
| `sessions/index.pending`, `index.json`, `summary.json` | Listing index and its pending marker. |
| `usage.jsonl` | Store-wide usage fact log (`coverage`, `generation`, `incident` records). |

Authoritative vs derived:

- **Authoritative**: `events.jsonl`, truncated to the commit boundary
  recorded in `commit.<log_generation>.json`.
- **Derived and rebuildable**: `checkpoint.json` (a fold of the log up to
  `through_seq`), `session.json` counters, `display.json`, `usage-v2.json`,
  the `latest/` pointer cache, and the sessions index. The binary carries a
  replay-fallback path for exactly this: `event=session_replay_checkpoint_
  fallback reason=ManifestStateMismatch` and `reason=StaleEventFileFinger
  print` [literal], i.e. a checkpoint that disagrees with the log is
  discarded and the log is replayed.
- **Not rebuildable from the log**: the spilled `tool-results/` bodies, the
  command replay tapes, and web-fetch artifacts. The log stores handles, not
  bytes.

Conceptual model: **session-as-directory wrapping a session-as-log**, with
the fold materialized next to the log as a cache.

## Keying and identity

Session id is a three-part string, and it doubles as the directory name
[observed]:

```text
<created_at_ms>-<created_at_unix_nanos>-<16 hex chars>
```

Both time components describe the same instant (milliseconds and
nanoseconds), so ids sort lexicographically by creation time within the same
epoch-digit width. There is no workspace component in the id; workspace
binding lives in fields, not in the key.

Workspace binding is a pair of fields carried in both the manifest and the
folded state [observed]: `origin_workspace_root` (where the session was
created) and `workspace_root` (where it is now). Relocation is a first-class
event rather than a re-key: `workspace_rebound` with payload
`{"previous_workspace_root": …, "workspace_root": …}` [literal]. The id is
stable across a moved working directory.

Ownership is asserted by `authority.json` [observed]:

```json
{"schema_version":1,"session_id":"…","authority_id":"<32 hex>",
 "storage_format":"event_log_v1","source":"native_create"}
```

`source` is one of `native_create`, `legacy_migration`, `watermark_advance`,
`log_generation_replace`, `legacy_to_v3`, `session_create` [literal], and a
staged `authority.pending.json` exists for transitions [literal]. `fx doctor`
surfaces failures of this file as `missing_authority`, `invalid_authority`,
`authority_transition`, `workspace_mismatch` [literal].

Listing is **workspace-scoped by default**: `fx sessions` enumerates the
current workspace only, and `--all` widens it [literal]. The per-workspace
pointer cache key is verified: the filename under `sessions/latest/` is the
hex `sha256` of the workspace root path with no trailing newline [observed,
reproduced].

## The store interface

fx exposes no pluggable store adapter. The operation contract below is
**reconstructed** from the on-disk effects, the CLI surface, and the
binary's diagnostic strings; there is no exported type to quote.

| Operation | Inputs | Effect and guarantee |
| --- | --- | --- |
| create | workspace root, preferences | Creates the session directory, writes `authority.json` with `source=native_create`, appends `session_started` at `seq=1` [observed]. |
| append | one event | Appends one JSON line to `events.jsonl`, `seq` strictly increasing [observed]. |
| commit | `through_seq`, `through_event_id`, `through_event_log_bytes` | Writes `commit.<log_generation>.json` via `commit.pending.json` + rename under `commit.lock` [observed + literal]. |
| checkpoint | folded state | Writes `checkpoint.json`; `session.json.checkpoint_sha256` is the `sha256` of that file's bytes [observed, reproduced]. |
| load/resume | session id | Reads manifest, validates fingerprint and checkpoint hash, else replays the log [literal]. |
| list | workspace root or `--all`, `--limit`, `--cursor` | Reads `display.json` sidecars/index; returns a `sessions` record with `count`, `sessions[]`, `has_more`, `next_cursor` [observed empty, literal for pagination]. |
| inspect | session id | `fx session <id> --json` → a `session_detail` projection [observed]. |
| migrate | session id | One-way legacy → `event_log_v1` [literal]. |
| recover | session id | Copies a recoverable corrupt session into a **new** session, source untouched [observed in help text]. |
| compact log | (none) | Rewrites the log under a new `log_generation`, leaving `events.compact.*` artifacts [literal]. |
| replace state | replacement id | Chunked base64 state replacement through three events (below) [literal]. |

There is no delete verb in the CLI. `fx doctor` mentions `cleanup_candidate`
/ `cleanup_removed` [literal], and manual removal is the documented recovery
step ("back up `~/.fx/sessions`, then inspect this session with
`fx session <id> --json`") [literal].

## Write and append path

- **Append-only, one JSON object per line.** No length framing; the log is
  plain JSONL [observed].
- **Ordering** is a `seq` field starting at 1, monotonic, dense in every
  observed log (720 and 1428 events with no gaps) [observed]. Each event
  also carries a 32-hex `event_id` and `timestamp_ms`.
- **Generation fencing.** Every event repeats `log_generation` (32 hex),
  which also names the commit file. A reader can detect a log rewritten
  under a new generation without reading the whole file [observed].
- **Durability.** The commit boundary is written separately from the log and
  records `through_event_log_bytes`, so a torn trailing append is detectable
  by byte length as well as by sequence. `session.json` additionally carries
  `event_log_stat_fingerprint` (opaque 32-byte hex) and `checkpoint_sha256`
  [observed]. Locks are zero-byte advisory files (`session.lock`,
  `commit.lock`, `latest.lock`) [observed]; the failure message is "session
  is busy or the filesystem cannot provide the required lock" [literal].
- **Concurrency.** Single-writer-per-session via those locks. No
  expected-version precondition on append is visible; the boundary is
  advanced after the fact, not asserted before it.
- **Backpressure on state size.** When folded state is too large to embed,
  fx switches to a three-event chunked replacement (see the event catalog),
  which is the only place a compare-and-swap-like validation appears
  (`state_replacement_committed` carries a `validation` field) [literal].

## Read and resume path

Resume prefers the checkpoint and falls back to replay [literal]:

1. Read `session.json`; check `event_log_stat_fingerprint` and
   `checkpoint_seq`/`checkpoint_sha256`.
2. If consistent, load `checkpoint.json.state` directly. The binary traces
   this as `event=session_replay source=… elapsed_us=…`.
3. On mismatch, emit `event=session_replay_checkpoint_fallback reason=
   ManifestStateMismatch | StaleEventFileFingerprint | …` and fold the log
   from `generation_base_seq` forward.
4. Trim to the commit boundary; events past `through_seq` are not trusted.

Resume is otherwise **eager**: the whole folded state including the full
history array is materialized (a 1.2 MiB `checkpoint.json` for a single-turn
session is normal, because tool results, diffs, and previous file contents
are inlined) [observed]. What is **lazy** is the spilled material: tool
outputs above the cap live in `tool-results/` and are pulled back through a
`read_tool_result` tool by handle, with byte-range access
(`<tool_result handle="" start_byte="" end_byte="" total_bytes="">`)
[literal]; command output is replayed from the `.bin` tapes, with explicit
degradation paths ("resume command replay fell back to raw result",
"resume command replay could not read result handle=") [literal].

`context_history_start` in the folded state is the index where the
model-visible window begins, distinct from the length of the durable history
array [observed].

## Listing, summaries, and search

- `display.json` is the write-time listing sidecar [observed]:

  ```json
  {"schema_version":1,"title":"<first user message, truncated>",
   "preview":"<full first user message>",
   "origin_workspace_root":"/workspace/root"}
  ```

  Fallback titles are `Untitled session` and `Image session` [literal].
- The `latest/<sha256(workspace_root)>.json` pointer is a two-field read
  model with an explicit lifecycle status [observed]:

  ```json
  {"schema_version":2,"status":"ready","workspace_root":"/workspace/root",
   "session_id":"…","updated_at_ms":1780000000000}
  ```

- A store-level index exists (`index.json`, `summary.json` shaped
  `{"schema_version":1,"count":…,"latest_id":…}`) with an `index.pending`
  marker (contents: the literal string `pending`) [observed + literal]. Index
  entries track `display_metadata_present` and `has_managed_children`
  [literal].
- Listing is **not** guaranteed exact: `fx doctor` reports "sessions: exact
  saved-session count unavailable without a full session scan" [observed],
  which places fx in the same directory-scan-plus-sidecar family as
  Grok Build and Claude Code rather than the indexed-query family.
- There is **no transcript search subsystem**. No FTS, no vector index, no
  content index of any kind appears on disk or in the string table.

## Entry and message structure

This is the core of this dossier: fx's durable message types, in the exact
wire spelling the binary emits.

### The event envelope

Every line of `events.jsonl` is [observed], with this key order:

```json
{"schema_version":1,"log_generation":"<32 hex>","seq":2,
 "event_id":"<32 hex>","timestamp_ms":1780000000000,
 "kind":"usage_checkpointed","payload":{ … }}
```

`schema_version` is per-event and is `1` in this build. The session manifest
carries its own `"schema_version": 3`, and the execution record inside a turn
carries a third, independent `schema_version`, three versioned layers.

### Event kind catalog

Eight kinds exist in the binary's kind table, contiguous and in this order
[literal]; three were observed live.

| kind | payload | evidence |
| --- | --- | --- |
| `session_started` | `{id, created_at_ms, origin_workspace_root, workspace_root, conversation_language, preferences{model,effort,fast_mode}, usage{…}}` | [observed] |
| `history_turn_committed` | `{conversation_language, total_input_tokens, total_output_tokens, turn: <HistoryTurn>}` | [observed] |
| `usage_checkpointed` | `{usage: <UsageSnapshot>}` | [observed] |
| `preferences_changed` | `{model, effort, fast_mode}` | [literal] |
| `workspace_rebound` | `{previous_workspace_root, workspace_root}` | [literal] |
| `state_replacement_started` | `{replacement_id, reason, encoded_bytes, sha256, chunk_count}` | [literal] |
| `state_replacement_chunk` | `{replacement_id, chunk_index, chunk_sha256, base64}` | [literal] |
| `state_replacement_committed` | `{replacement_id, validation}` | [literal] |

Note the granularity: **one event per committed turn**, not per message or
per tool call. A 15-minute session with dozens of tool calls produced exactly
one `history_turn_committed` and 718 `usage_checkpointed` events [observed].
Usage accounting is the chatty part of this log; the conversation is not.

### HistoryTurn: the four message types

`history` in the folded state is an array of tagged turns. `kind` is always
the first key emitted [literal, confirmed by the emission fragments
`{"kind":"assistant","user":`, `{"kind":"background_command","user":`,
`{"kind":"interrupted","user":`, `{"kind":"compacted_summary","summary":`].

**1. `assistant`**: the normal completed turn [observed]:

```json
{"kind":"assistant",
 "user":{"text":"…","images":[]},
 "assistant":"…final assistant markdown…",
 "execution":{"schema_version":3,"tool_steps":[…],"files":[…]}}
```

**2. `background_command`**: a turn whose work was handed to a background
process [literal]:

```json
{"kind":"background_command","user":{…},"background_record_id":"…"}
```

The record id points into the background-command store surfaced by
`fx background [last|<id>]`, whose fields are `managed_session`,
`managed_log_name`, `external`, `path`, `background_record_id`,
`process_token`, `log_storage`, `log_path`, `expect_url`, `server_url`,
`started_at_ms`, `exit_code`, `state`, `diagnostic` [literal].

**3. `interrupted`**: a cancelled turn, preserved rather than dropped
[literal]:

```json
{"kind":"interrupted","user":{…},"assistant":"…partial…",
 "tool_call":{…},"completed_tool_names":["read_file", …],
 "cancelled_command":{…}}
```

The runtime counterpart is visible in the trace format
`interrupted_turns= history_turn_kinds= projected_message_roles=
partial_interrupted_closures=` [literal]: interrupted turns are re-projected
into provider messages at resume rather than elided.

**4. `compacted_summary`**: the compaction marker [literal]:

```json
{"kind":"compacted_summary","summary":"…","removed_turn_count":N,
 "compaction_count":M}
```

### UserMessage

```json
{"text":"…","images":[{"encoding":"base64","data":"…","media_type":"…",
 "snapshot_path":"…","snapshot_sha256":"…"}]}
```

`text` and `images` are [observed] (`images` empty in all local sessions).
The image member fields are [literal]; note the two storage strategies side
by side, either inline `base64` **or** a `snapshot_path` + `snapshot_sha256`
reference into the store's `images/` area, with a
`legacy_snapshot_directory_cleanup_failed` diagnostic for the older layout
[literal]. A `work_id` field is emitted in this same literal neighborhood and
the folded state carries `last_subagent_work_id` [literal]; the owning record
for `work_id` is not determinable from the string table alone.

### ExecutionRecord

Two versions ship simultaneously and mean different things [observed]:

- `"schema_version": 3` is what `checkpoint.json` stores durably: it
  includes `files` and the full presentation payloads.
- `"schema_version": 2` is what `fx session <id> --json` emits: the same
  record with only `command_output_replay` and `command_process_presentation`
  **dropped** from tool results [observed]. `files`, `output_handle`,
  `preview`, and `committed_file_presentation` are all retained. A
  field-level diff of `checkpoint.json` against the CLI output for the same
  session (this build, a session that exercised `run_command`, 211 steps /
  263 results / 234 file entries, identical counts and key sets on both
  sides except those two keys) settles this; an earlier revision of this
  dossier claimed all five fields were dropped, which the diff refutes. The
  projection is documented as a beta read contract in the
  [session detail JSON reference](./session-detail-json-reference.md).

```json
{"schema_version":3,
 "tool_steps":[{"assistant":"…or null…",
                "tool_calls":[…],"tool_results":[…]}],
 "files":[…]}
```

`assistant` inside a step is the interstitial narration for that step and is
`null` for most steps; the turn-level `assistant` field holds the final
message [observed].

### ToolCall

```json
{"id":"chatcmpl-tool-…","name":"read_file",
 "arguments_json":"{\"path\":\"…\"}","provider_result":null}
```

All four fields [observed]. Arguments are stored as an **unparsed JSON
string**, and fx has a repair path for invalid ones
(`persisted_tool_arguments_repaired`, "Tool arguments were not valid JSON.")
[literal]. `provider_result` was `null` in every observed call and is
presumably the provider-native result passthrough (see `provider_native`
below).

### ToolResult

Fifteen fields, all [observed] in one record set:

```json
{"tool_call_id":"chatcmpl-tool-…",
 "tool_name":"read_file",
 "status":"success",
 "output":"<tool_result_preview handle=\"result-read_file-…txt\" …>",
 "output_handle":"result-read_file-<16hex>-<16hex>.txt",
 "preview":"<path>…</path>…",
 "output_bytes":17798,
 "stored_output_bytes":17798,
 "truncated":true,
 "provider_native":false,
 "created_at_ms":1780000000000,
 "permission_feedback":[],
 "committed_file_presentation":null,
 "command_output_replay":null,
 "command_process_presentation":null}
```

Population rates across 433 observed results: `output_handle` and `preview`
non-null in 9, `committed_file_presentation` in 13, `command_output_replay`
in 34, `command_process_presentation` in 1. The three specialised payloads
are mutually exclusive by tool: replay and process presentation only ever
appear on `run_command`, committed presentation only on file-writing tools.

- `status` observed as `success` and `failure`; the string table also holds
  `cancelled`, `denied`, `rejected`, `not_attempted`, `not_found`,
  `unavailable`, `deny` [literal], not all necessarily from this enum.
- `output` is the **model-facing** value. When a result is spilled, the
  stored `output` is a preview envelope pointing at the handle, and the full
  bytes live in `tool-results/`. The model is told: "Full redacted result is
  stored outside session JSON. Use `read_tool_result` with this handle to
  inspect a byte range or literal query." [literal]. Handles are declared
  session-scoped: `ResultHandleNotFound` says "handles are session-scoped and
  must be copied exactly" [literal].
- `output_handle` is a plain filename string [observed]. A separate
  availability-tagged handle shape exists for command artifacts
  (`{"kind":"available","handle":…}` / `{"kind":"unavailable"}` next to
  `command_artifact_handle`) [literal].
- `output_bytes` vs `stored_output_bytes` distinguishes produced size from
  retained size; `truncated` flags the model-facing cap
  (`model-facing tool result truncated label= cap_bytes=`) [literal].
- `permission_feedback` is an array, empty in all observed records; the
  prompt-side counterparts are `trusted_user_permission_feedback:` and
  `omitted_trusted_user_permission_feedback:` [literal].
- `command_output_replay` is the availability-tagged pointer to the terminal
  replay tape for a `run_command` result [observed]:
  `{"kind":"available","handle":"fx-command-replay-<pid>-<nanos>-<idx>-<16hex>.bin","framed_bytes":440}`.
  The tape itself lives outside the session directory (magic `FXRPLY01`), so
  a session's terminal replay is a **dangling reference** once the tape is
  gone; the complementary `{"kind":"unavailable"}` variant exists [literal].
- `command_process_presentation` records how the process ended [observed]:
  `{"kind":"exit_code","value":1}`. The binary also carries
  `{"kind":"signal","value":` [literal]. It is `null` for successful commands
  in the observed set, so absence encodes "nothing worth showing", not
  "unknown".

### CommittedFilePresentation

The durable, replayable diff for an edit [observed]:

```json
{"path":"apps/example/test/example_test.exs",
 "kind":"edited",
 "lines":[{"kind":"addition","old_line":null,"new_line":171,"text":"    end"},
          {"kind":"elision","old_line":null,"new_line":null,
           "text":"⋯ +54 omitted"}],
 "additions":59,"deletions":0,"truncated":true,
 "previous_content":"…entire prior file content…",
 "after_content":null,
 "lifecycle_id":{"turn_id":2,"call_id":"chatcmpl-tool-…"}}
```

- Line `kind` observed as `addition`, `deletion`, `context`, `elision`; the
  binary's contiguous enum block is `present`, `context`, `addition`,
  `deletion`, `elision`, `notice` [literal].
- File `kind` observed as `edited`; the adjacent literal block is `added`,
  `edited`, `delete`, `rename`, `copy`, `search` [literal]. `new_path` exists
  for the rename case [literal].
- `previous_content` stores the **entire prior file content inline**, so a
  session that edits large files carries full pre-images in its checkpoint
  and in the committed turn event. This is the single largest contributor to
  session size observed (a 1.2 MiB checkpoint for one turn).
- `truncated` here refers to the rendered `lines` list, not the content.

### File lifecycle entries (`execution.files`)

A per-turn ledger of every file the turn touched, separate from the diffs
[observed]; 340 entries across two local sessions:

```json
{"path":"apps/example/lib/example.ex","new_path":null,
 "action":"read","status":"success","stale":false,
 "tool_call_id":"chatcmpl-tool-…","tool_name":"read_file",
 "model_view_covers_full_file":true}
```

`action` observed: `read`, `edit`, `write`, `list`, `search`, `delete`.
`status` observed: `success`, `failure`. `stale` is a boolean marking a read
whose content was later invalidated (16 of 192 reads in one session)
[observed]. `model_view_covers_full_file` records whether the model saw the
whole file or a window; this is a freshness/consistency ledger for
edit safety, kept in the durable record rather than recomputed.

### UsageSnapshot

Two shapes with the same name, and the difference matters [observed].

The **in-log** `usage` payload (carried by every `usage_checkpointed` event
and embedded in `checkpoint.json`) has **no `schema_version`** and 18 keys:
`billing` (`complete` | `incomplete`), `api_duration_complete`,
`wall_duration_complete`, `code_complete`, `next_sequence`,
`settled_through_sequence`, `api_duration_ms`, `wall_duration_ms`,
`total_cost`, `input_tokens`, `output_tokens`, `cache_read_tokens`,
`cache_write_tokens`, `billable_web_search_calls`, `lines_added`,
`lines_removed`, `models[]` (each with `model`, `first_sequence`,
`input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`,
`billable_web_search_calls`, `total_cost`), `pending[]`.

The **sidecar** `usage-v2.json` `snapshot` is a superset: `schema_version: 2`
plus `reasoning_tokens`, `request_count`, `publication_backlog[]`, and
`incidents[]`. So the sidecar is not a pure fold of the log; it carries
counters and a publication ledger the log never records. Rebuilding
`usage-v2.json` from `events.jsonl` alone would lose four fields.

`pending[]` entries are `{"id":…,"origin":…,"sequence":N,"team":…}`, the
not-yet-published billing units [observed, 470 across the local logs]. The
`settled_through_sequence` / `next_sequence` pair plus `pending` and
`publication_backlog` make this a small ledger with its own settlement
watermark, folded from the 718 `usage_checkpointed` events. It is the reason
the event log is chatty: 1434 of 1436 observed events are usage
checkpoints, against 2 turn commits.

### CLI projection types

`--json` output is its own tagged family, distinct from the storage types
[observed / literal]: `session_detail`, `sessions`, `session_summary`,
`session_migration`, `session_recovery` (with `source_id`, `recovered_id`,
and outcomes `recovered`, `recovered_with_unverified_artifacts`,
`indeterminate`), `workspace`. The `session_detail` shape is documented as a
beta read contract in the
[session detail JSON reference](./session-detail-json-reference.md),
including an error envelope (`{"kind":"session","error":…,"code":…}`) this
dossier had not catalogued.

```json
{"kind":"session_detail","id":"…","created_at_ms":…,"updated_at_ms":…,
 "history_len":1,"conversation_language":"und-Latn","history":[…]}
```

### Versioning and evolution

Four independently versioned layers [observed]: session manifest
(`schema_version: 3` + `storage_format: event_log_v1`), event envelope
(`schema_version: 1`), execution record (`schema_version: 3` durable /
`2` projected), usage snapshot (`schema_version: 2`). Sidecars carry their
own (`display.json` v1, `latest/*.json` v2).

Legacy formats are named and sniffed, not silently read: `legacy_snapshot_v1`,
`legacy_snapshot_v2`, `session.legacy.json`, plus `legacy_v1`, `legacy_v2`,
`current`, `stale`, `missing`, `invalid`, `unsupported_schema` as
classification outcomes [literal]. The ratchet is **one-way and explicit**:
`fx session migrate <id>` with a size guard (`legacy_too_large`,
`oversized_legacy_snapshot`, `--allow-large`), and the migration operation is
itself journaled as `{"kind":"legacy_to_v3","authority_id":…,"prior":
{"storage_format":"legacy_snapshot_v…","primary_bytes":…,"primary_sha256":…},
"proposed":{"storage_format":"event_log_v1","log_generation":…,
"through_event_id":…,"through_event_log_bytes":…}}` [literal]. Failure is
non-destructive by contract: "migration did not complete because resources
were exhausted; the original session remains authoritative" [literal].

## Compaction and history management

Two distinct compactions exist and should not be conflated:

1. **Conversation compaction**: a `compacted_summary` turn replaces removed
   turns in the model-visible window, carrying `removed_turn_count` and a
   cumulative `compaction_count` [literal]. The window boundary is the
   `context_history_start` index in the folded state [observed]; the durable
   `history` array is not shortened by it (our inference from those two
   facts).
2. **Log compaction**: the event log itself is rewritten under a new
   `log_generation`, leaving `events.compact.*` artifacts and a diagnostic
   `event=canonical_log_compaction_failed kind= current_bytes= growth_bytes=
   growth_frames=` [literal]. The advice string "inspect the failed
   compaction artifact; keep it until no writer is active" [literal] implies
   the rewrite is staged, not in place.

The chunked `state_replacement_*` triple is the third mechanism: when the
folded state must be re-seated wholesale, it is base64-chunked across events
with per-chunk `chunk_sha256` and a whole-payload `sha256`, then committed
with a `validation` result [literal]. That keeps even a full-state rewrite
inside the append-only log.

## Rewind, checkpoints, and fork

- **No rewind or branch** verb exists in the CLI or the string table. The
  retroactive operations fx has are recovery and migration, not history
  editing.
- **Checkpoints** are storage checkpoints (state folds), not user-facing
  turn checkpoints. `fx doctor` describes the state directory as
  "per-session managed state created on demand" [observed].
- **Fork** appears only as `fx session recover <id>`, which is
  copy-plus-new-identity: it "creates a separate resumable copy and leaves
  the source unchanged", refuses when the source has a valid commit boundary
  ("resume it normally"), and records `{"kind":"session_recovery",
  "source_id":…,"recovered_id":…}` with outcome `recovered`,
  `recovered_with_unverified_artifacts`, or `indeterminate` [literal]. The
  lineage link is that record, not a field in the new session.
- File-state checkpointing is per-edit and inline (`previous_content` /
  `after_content` on each committed presentation), with no dedup or content
  addressing [observed].

## Subagents and nested sessions

Partially reconstructable. Subagent state is **not** a nested session
directory: the parent session holds `subagent/relationship-index.bin` plus
`relationship-page-*` pages, a binary format [observed], and the folded state
carries `last_subagent_work_id` [literal]. Live coordination runs over a
named channel family: `fx.subagent.relationship-request.v1`,
`relationship-approval.v1`, `approval.v1`, `approval-identity.v2`,
`prepared-permission.v1`, `main-approval.v1`, `operation-request.v1`,
`operation-effect.v1`, `delivery.v1`, `interval-delivery.v1`,
`tool-activity.v1`, `live-authority.v1`, `host-authority.v1`,
`work_notifications` [literal], with repair diagnostics
("relationship projection repair pending child_id=") indicating the index is
a projection with its own repair path. The index listing tracks
`has_managed_children` [literal], so parent→child is a durable relation. The
child transcript's own storage location was not determined.

## Retention, deletion, and multi-host

- **No TTL, no janitor, no retention policy** for sessions is visible: no
  age cap, no session-count cap, no scheduled cleanup in the string table.
  The retention concepts that do exist are in-session output caps
  ("command output retention cap reached retained= cap=; subsequent records
  count only") and an `<artifact_retention>` prompt block [literal].
- **Deletion** has no CLI verb. `cleanup_candidate` / `cleanup_removed` /
  `report_only=` diagnostics exist for orphaned staging directories
  [literal], and the operator instruction is to back up and inspect
  manually.
- **Multi-host** is out of scope by construction: the whole design assumes a
  local POSIX filesystem with working advisory locks. The failure mode is an
  explicit refusal rather than a fallback: "durable session storage is
  unsafe or does not support required private permissions",
  `durable_path_unsafe`, `unsafe_initial_shape`, `unsafe_verified_shape`,
  `changed_during_open`, `unsafe_path` [observed in `fx doctor` output +
  literal]. fx checks the directory's permission bits and refuses to treat
  an unsafe location as durable.

## Interop with foreign session stores

None found. fx reads no other product's session store, and no import or
foreign-resume path appears in the CLI or the string table.

## What this implies for our Session Store (our inference)

fx is the cleanest small-scale statement of the pattern we are targeting: a
per-session append-only log named `event_log_v1`, a fold cached beside it
with a content hash so a stale fold is detectable and discardable, and an
explicit durable commit boundary separate from the log file so a torn tail is
recoverable by byte length as well as by sequence. Three things are worth
importing directly:

- **The commit boundary as a separate artifact.** `through_seq` +
  `through_event_id` + `through_event_log_bytes` gives a reader three
  independent ways to detect a partially written tail. Our stream-position
  equivalent should carry the same redundancy at the projection boundary.
- **Generation fencing inside every event.** Repeating `log_generation` per
  record makes a rewritten/compacted log self-identifying at any offset. It
  is the cheap version of what our event store gets from stream identity.
- **Turn-granular commits with a separate accounting stream.** One
  `history_turn_committed` per turn keeps the conversation stream small and
  semantically meaningful, while 700+ usage events churn in the same log.
  For us that argues for splitting the accounting facts out of the
  conversation stream rather than replicating fx's single-log mixing.

Two costs are equally instructive as anti-patterns: **full pre-image inlining**
(`previous_content` per edit, no dedup, no content addressing, megabyte
checkpoints for a single turn) and the **underversioned public projection**:
the `schema_version: 2` execution record emitted by `fx session <id> --json`
drops
`command_output_replay` and `command_process_presentation` (so the public JSON
has no structured exit code or signal), omits the session-level metadata the
store holds (workspace, model, effort, token totals, title, preview), and the
top-level response carries no `schema_version` of its own, only the nested
execution object's. The projection is at least a documented beta contract now
(see the [session detail JSON reference](./session-detail-json-reference.md)),
but the emitted JSON can also contain unescaped control characters in strings,
which strict JSON parsers reject [observed]. If we expose a read API over the
Session Store, its projection needs to be a documented contract versioned at
the top level, emitting strictly valid JSON, rather than an older internal
shape reused as the public one.

## Open questions

- The payload shapes for `preferences_changed`, `workspace_rebound`, and the
  three `state_replacement_*` events are literal-derived; field presence is
  certain, field order and optionality are not verified against a live log.
- The owning record for the `work_id` literal (user message, image, or tool
  call) is undetermined, as is how `last_subagent_work_id` links to it.
- Where a subagent's own transcript is stored, and whether a child is a
  sibling session directory or lives entirely inside the parent's binary
  relationship pages.
- Whether the replay tapes referenced by `command_output_replay` are ever
  garbage collected, and what a reader is expected to do with a session whose
  tapes are gone (the `{"kind":"unavailable"}` variant exists, but no
  transition from `available` to it was observed).
- Whether `history` is truly never shortened across a compaction, or whether
  the durable array is rewritten via `state_replacement_*`. Both readings fit
  the available evidence.
- No retention or deletion policy was found. Whether unbounded session growth
  is intentional or simply unimplemented at this version is unknown.
- `event_log_stat_fingerprint` is an opaque 32-byte hex value; its inputs
  (inode, size, mtime) were not determined.

## Appendix: full structural inventory

Every path reachable in the durable artifacts, with the JSON type and the
observed occurrence count across the local corpus (four sessions with a
`checkpoint.json`, 1436 log events, 2 committed turns, 368 tool steps, 433
tool calls, 433 tool results, 340 file ledger entries, 68 diff lines). Counts
are the *number of times the path occurred*, so a `null` and a typed row for
the same path give the exact nullability split. This is the complete surface
a reader has to handle, and it is the ground truth behind the shape
descriptions above [observed].

### `session.json` (manifest, one per session)

| Field | Type | Note |
| --- | --- | --- |
| `schema_version` | number | `3` |
| `storage_format` | string | `event_log_v1` |
| `id` | string | `<created_ms>-<created_nanos>-<16 hex>` |
| `authority_id` | string | 32 hex, single-writer fence |
| `log_generation` | string | 32 hex, changes on log compaction |
| `created_at_ms`, `updated_at_ms` | number | epoch ms |
| `origin_workspace_root`, `workspace_root` | string | rebind keeps both |
| `conversation_language` | string | BCP-47 subtag, `und-Latn` observed |
| `history_len` | number | committed turn count |
| `total_input_tokens`, `total_output_tokens` | number | denormalised counters |
| `last_event_seq` | number | log head |
| `event_log_bytes` | number | size guard input |
| `event_log_stat_fingerprint` | string | 64 hex, opaque |
| `generation_base_seq`, `generation_base_bytes` | number | where this generation starts |
| `checkpoint_seq` | number | fold watermark |
| `checkpoint_sha256` | string | verified equal to `sha256(checkpoint.json)` |
| `preferences` | object | `{model, effort, fast_mode}` |

### `checkpoint.json` (folded state, one per session)

| Path | Type | Count |
| --- | --- | --- |
| `schema_version`, `session_id`, `log_generation` | number, string, string | 4 |
| `through_seq`, `through_event_id`, `through_event_log_bytes` | number, string, number | 4 |
| `state.id`, `state.created_at_ms`, `state.updated_at_ms` | string, number, number | 4 |
| `state.workspace_root`, `state.origin_workspace_root` | string | 4 |
| `state.conversation_language` | string | 4 |
| `state.context_history_start` | number | 4 |
| `state.total_input_tokens`, `state.total_output_tokens` | number | 4 |
| `state.preferences.{model,effort,fast_mode}` | string, string, boolean | 4 |
| `state.usage` | object | 4 (same shape as the in-log usage payload) |
| `state.history[]` | object | 2 |

`state.context_history_start` is the only field that encodes "the model does
not see the whole history"; it is an index into `state.history`, not a
deletion. Nothing else in the checkpoint is a window or a projection.

### `events.jsonl` envelope and payloads

| Path | Type | Count |
| --- | --- | --- |
| `schema_version`, `seq`, `timestamp_ms` | number | 1436 |
| `event_id`, `log_generation`, `kind` | string | 1436 |
| `payload` | object | 1436 |
| `payload.usage` (`usage_checkpointed`) | object | 1434 |
| `payload.turn` (`history_turn_committed`) | object | 2 |
| `payload.total_input_tokens`, `payload.total_output_tokens` | number | 2 |
| `payload.id`, `payload.created_at_ms`, `payload.workspace_root`, `payload.origin_workspace_root`, `payload.preferences` (`session_started`) | mixed | 6 |
| `payload.conversation_language` | string | 8 |

The 1434:2 ratio between usage checkpoints and turn commits is the single
most important number in this dossier. fx's event log is dominated by a
billing ledger, not by conversation.

### `payload.turn` (a committed `HistoryTurn`)

| Path | Type | Count |
| --- | --- | --- |
| `turn.kind`, `turn.assistant` | string | 2 |
| `turn.user.text` | string | 2 |
| `turn.user.images[]` | array | 2, empty in all |
| `turn.execution.schema_version` | number | 2 (`3`) |
| `turn.execution.tool_steps[]` | object | 368 |
| `tool_steps[].assistant` | string / null | 236 / 132 |
| `tool_steps[].tool_calls[]` | object | 433 |
| `tool_calls[].{id,name,arguments_json}` | string | 433 |
| `tool_calls[].provider_result` | null | 433 |
| `tool_steps[].tool_results[]` | object | 433 |
| `tool_results[].{tool_call_id,tool_name,status,output}` | string | 433 |
| `tool_results[].{output_bytes,stored_output_bytes,created_at_ms}` | number | 433 |
| `tool_results[].{truncated,provider_native}` | boolean | 433 |
| `tool_results[].permission_feedback[]` | array | 433, empty in all |
| `tool_results[].output_handle` | string / null | 9 / 424 |
| `tool_results[].preview` | string / null | 9 / 424 |
| `tool_results[].committed_file_presentation` | object / null | 13 / 420 |
| `tool_results[].command_output_replay` | object / null | 34 / 399 |
| `tool_results[].command_process_presentation` | object / null | 1 / 432 |
| `turn.execution.files[]` | object | 340 |
| `files[].{path,action,status,tool_call_id,tool_name}` | string | 340 |
| `files[].new_path` | null | 340 |
| `files[].{stale,model_view_covers_full_file}` | boolean | 340 |

`output_handle` and `preview` are populated together, always: spilling a
result and previewing it are one decision, not two.

### Nested payloads

| Path | Type | Count |
| --- | --- | --- |
| `command_output_replay.{kind,handle}` | string | 34 |
| `command_output_replay.framed_bytes` | number | 34 |
| `command_process_presentation.kind` | string | 1 |
| `command_process_presentation.value` | number | 1 |
| `committed_file_presentation.{kind,path}` | string | 13 |
| `committed_file_presentation.{additions,deletions}` | number | 13 |
| `committed_file_presentation.truncated` | boolean | 13 |
| `committed_file_presentation.after_content` | string | 13 |
| `committed_file_presentation.previous_content` | string / null | 12 / 1 |
| `committed_file_presentation.lifecycle_id.turn_id` | number | 13 |
| `committed_file_presentation.lifecycle_id.call_id` | string | 13 |
| `committed_file_presentation.lines[].{kind,text}` | string | 68 |
| `lines[].old_line` | number / null | 23 / 45 |
| `lines[].new_line` | number / null | 47 / 21 |
| `usage.models[]` | object | 2739 |
| `usage.models[].model` | string | 2739 |
| `usage.models[].{first_sequence,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,billable_web_search_calls,total_cost}` | number | 2739 |
| `usage.pending[].{id,origin,team}` | string | 470 |
| `usage.pending[].sequence` | number | 470 |

`lifecycle_id` is a composite key `{turn_id, call_id}`, not a string: a
committed file edit is identified by the turn it happened in plus the tool
call that made it. That pair is fx's only cross-record correlation key
besides `tool_call_id`.

### Tool catalog

Tool names observed in stored calls, with counts: `read_file` 194,
`run_command` 87, `grep_files` 74, `list_files` 55, `edit_file` 9,
`write_file` 4, `read_tool_result` 4, `glob_files` 3, `semantic_search` 1,
`file_info` 1, `delete_file` 1. `read_tool_result` is notable: it is the
model-facing tool for pulling a spilled result back in by handle, so the
claim-check indirection is part of the tool surface, not just the storage
layer.
