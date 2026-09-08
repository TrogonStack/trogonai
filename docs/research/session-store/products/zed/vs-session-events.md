# Zed compared to our session event catalog

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT_COMPARISON](../../RESEARCH_PROMPT_COMPARISON.md).
Stage-one dossier: [Zed](./index.md).
Compared against `proto/trogonai/session/sessions/v1alpha1/` and [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) on 2026-08-04.

**Store maturity: 11/12** -- evolution scars 3/3 (`sqlez::Connection::migrate`
stores each migration's formatted SQL text and hard-fails on drift,
`crates/sqlez/src/migrations.rs:37-104`, with its own regression test
`changed_migration_fails`, `crates/sqlez/src/migrations.rs:311-346`; separately,
`DbThread::VERSION = "0.3.0"` is checked against the JSON `"version"` field and
falls back to `upgrade_from_agent_1` against `crate::legacy_thread::SerializedThread`
for any older value, `crates/agent/src/db.rs:189`, the dossier's [Entry/message structure and versioning](./index.md#entrymessage-structure-and-versioning) --
two independent, format-version-carrying evolution mechanisms is strong
evidence by the axis's own bar, even though one of them has no ledger; see
"Resolving the maturity tension" below), operational age 2/3 (crash-safety
code exists -- `PRAGMA journal_mode=WAL; PRAGMA busy_timeout=500;`,
`crates/db/src/db.rs:130-131`; `cx.on_app_quit(Self::flush_threads_on_quit)`,
`crates/agent/src/agent.rs:575,1795`; an `open_fallback_db` crash-recovery path,
`crates/db/src/db.rs:215` -- but the dossier could not find a first-commit date for the store
or a filed corruption/growth/lock-contention issue to cite, so this is scored
on inferred failure-mode-driven design, not documented incidents), exposure
3/3 (vendor-shipped desktop editor with four isolated release channels --
Stable/Preview/Nightly/Dev, each its own `db.sqlite`, `crates/db/src/db.rs:164-167`
-- plus first-class SSH/WSL/Docker remote-project handling via
`RemoteConnectionIdentity`, `crates/remote/src/remote_identity.rs:10-27`),
design independence 3/3 (the agent thread store -- `crates/agent`, `crates/agent_ui`,
`crates/sqlez` -- is original Zed engineering, not inherited from an upstream
fork; the dossier found no fork-parent code to diverge from).

This score sits one point below OpenCode/T3 Code-class evidence (not
established in this document; see [synthesis.md](../../synthesis.md)) but is
the highest-scoring **document store** in the corpus by this rubric -- most of
its strength comes from evolution scars and exposure, not from being
event-sourced, which it explicitly is not (see below). Where Zed disagrees
with a higher-scoring event-sourced precedent (T3 Code, OpenCode), those
precedents are the default per the scoring rule; where Zed is the *only*
product in the corpus that actually cascade-deletes subagents (see
[the two gaps the industry has not closed](#the-two-gaps-the-industry-has-not-closed)), its 11/12 score is why that evidence
is weighted heavily here despite Zed not being a log-shaped store.

### Resolving the maturity tension

The task framing is right to flag a tension: Zed is the oldest codebase
studied, yet `ThreadsDatabase` -- the table holding actual message content --
has no migration ledger at all, only best-effort `ALTER TABLE ... ADD COLUMN`
with **errors swallowed if the column already exists**
(`crates/agent/src/db.rs:456-471`). Averaging that against `ThreadMetadataDb`'s
strict ratchet into a single "evolution scars" number would erase the most
important fact in the dossier. The axis asks whether the store format changed
under load and carried its data forward -- it did, twice, by two deliberately
different mechanisms for two deliberately different reliability
requirements: metadata identity/structure (which must never silently drift)
got the hard-fail ratchet; message content and per-thread feature flags
(which change shape often) got additive `#[serde(default)]` fields and an
error-swallowing `ALTER TABLE`. That asymmetry *is* the evidence, not a
disqualifying inconsistency -- it is the strongest real-world precedent in
this corpus for treating a session's envelope/ordering schema and its
payload schema as two different reliability problems, which is exactly what
[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3 already does (`LEGACY_REQUIRED` presence plus `reserved`
retired field numbers on the envelope and event registration, versus purely
additive optional payload fields with no per-event version branch). See
[recommendation 2](#2-add-an-automated-drift-ratchet-for-the-sessionevent-envelope-and-oneof-registration)
for where our side of that asymmetry is currently a convention, not an
enforced contract, unlike Zed's ratchet.

## The one structural difference everything else follows from

Zed's agent thread store is a **reactive, whole-document, last-write-wins
overwrite**, not a log. `crates/agent/src/agent.rs:820` registers
`cx.observe(&thread_handle, ...)` on every `Thread` entity, and that observer
calls `save_thread` (`crates/agent/src/agent.rs:1736`) on **essentially every GPUI change
notification** -- a new message, a tool-call update, a title change, all
trigger the same path. `ThreadsDatabase::save_thread_sync`
(`crates/agent/src/db.rs:489`) does a full JSON+zstd blob upsert of the
*entire* `messages: Vec<Arc<DbMessage>>` on every one of those saves; there is
no per-turn append record, no per-fact append record, and no expected-version
precondition anywhere in the write path (the dossier's [Write and append path](./index.md#write-and-append-path-ordering-durability-concurrency-delivery)). Our
catalog commits at **fact granularity**: `UserMessageRecorded`,
`ToolCallRequested`, `ToolCallCompleted`, and 38 further arms are separate
append-only events, each durable the instant it lands, each classified under
one of `NoStream`/`At`/`Any` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 2).

This single fact is the root of every other divergence in this document, not
a coincidence alongside them:

- **Rewind is destructive because the unit of write is the whole document.**
  `Thread::truncate` (`thread.rs:2359-2383`) does
  `for message in self.messages.drain(position..)` -- an in-memory `Vec::drain`
  -- and the very next reactive save overwrites `threads.db` with that
  shortened vector. There is nothing to append a marker *to* that would
  survive the overwrite; the only way to "keep" a rewound tail would be to
  never overwrite the document, which is not how this store works. Our
  `SessionRewound.keep_through` (`proto/trogonai/session/sessions/v1alpha1/session_rewound.proto:18`)
  is a fact appended *alongside* the untouched history, because history is
  never overwritten in the first place ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 2/6).
- **Compaction survives only because its marker happens to live inside the
  same document that gets overwritten, not because it is durable by design.**
  `Message::Compaction(CompactionInfo::Summary(...))` is inserted into
  `self.messages` (`thread.rs:3216-3234`) and is retained on every subsequent
  save simply because nothing removes it from the vector before the next
  overwrite. It is durable by omission, not by a contract that says "this
  fact, once appended, is never edited." Our `Compacted` event
  (`proto/trogonai/session/sessions/v1alpha1/compacted.proto:19`) is durable by
  the same append-only guarantee every other event gets -- the two behave
  identically in Zed today only because nothing has yet exercised the case
  where compaction *should* be reverted, which truncate's existence shows is a
  real user operation.
- **There is no expected-version precondition because there is nothing
  partial to guard.** A whole-document overwrite has only one meaningful
  concurrency question -- "did I have the latest version when I overwrote?" --
  and Zed answers it with SQLite's WAL mode plus a 500ms busy-timeout
  (`crates/db/src/db.rs:130-131`), not a compare-and-swap. Our per-command
  `WRITE_PRECONDITION` classification (`NoStream`/`At`/`Any`, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 2)
  exists precisely because our unit of write is a single fact that can
  conflict with another single fact, a problem a whole-document store does
  not have in the same shape.
- **Resume is all-or-nothing because there is no cursor into a document.**
  `ThreadsDatabase::load_thread` (`crates/agent/src/db.rs:607`) is a single-row `SELECT`,
  zstd-decompress, then a full deserialize of the entire message vector in one
  shot (the dossier's [Read and resume path](./index.md#read-and-resume-path)). Our aggregate resumes from the newest
  snapshot and replays only the tail after it ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 8) because the
  underlying representation is a sequence of facts with a position, not one
  blob.

Zed is, in other words, a genuinely mature **mutable-document store** wearing
ACP's identity vocabulary (see [Semantic mismatches](#semantic-mismatches)),
not an approximation of event-sourcing the way the JSONL-transcript products
in the wider corpus are. That makes it a cleaner contrast than fx's
turn-granular commit: fx at least closes a write once per turn; Zed closes
one once per *any* observable mutation of the whole thread.

## Mapping

| Zed | Ours | Verdict |
| --- | --- | --- |
| `ThreadMetadataDb`/`sidebar_threads` (title, timestamps, project paths, remote scoping, archival flag) | Fold of `SessionStarted`/`SessionRenamed`/`SessionArchived`/`SessionUnarchived`/`SessionHidden` into `SessionProjection` (ADR facet 8) | Ours (a rebuildable projection, not a separately-maintained row that can drift from the log) |
| `ThreadsDatabase`/`threads` (one full JSON+zstd blob per thread) | The `SessionEvent` stream itself, fact-per-event | Ours, decisively (see structural difference above) |
| `ThreadId` (newtype `uuid::Uuid`) + `ThreadMetadata.session_id: Option<acp::SessionId>` + in-memory reverse index `threads_by_session` | `SessionId`, opaque, minted atomically at `CreateSession`'s `NoStream` batch; no pre-session identity exists | Semantic mismatch -- see below |
| `acp::SessionId` as storage key for `ThreadsDatabase` | `SessionId`, mapped to subject `session.sessions.events.<session_id>` by a `StreamSubjectResolver` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 1) | Semantic mismatch -- see below |
| `DbThread.messages: Vec<Arc<DbMessage>>` with `Message::{User,Agent,Resume,Compaction}` | `UserMessageRecorded`, `AssistantMessage{Started,Completed,Failed}`, `ToolCall{Requested,Started,Completed,Failed}` as separate append-only facts | Ours (fact granularity) |
| `Message::Compaction(CompactionInfo::Summary)`, retained in-vector, request view derived by `latest_compaction_message_ix_before` scan | `Compacted{summary_content, covers_from, covers_through: SessionOrdinal, trigger, usage}` | Equivalent design; validates [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 4 |
| `Thread::truncate` → `self.messages.drain(position..)`, destructive | `SessionRewound{keep_through: SessionOrdinal}` (inclusive), non-destructive | Ours, decisively |
| `SubagentContext{parent_thread_id, depth: u8}`, `MAX_SUBAGENT_DEPTH = 1`, sibling row, hidden by `parent_session_id.is_some()` filter | `ParentLinked{parent_session_id, parent_dispatched_at: SessionOrdinal, cascade_policy, operation_id}` + `DelegationDispatched` on the parent, list-time projection filter ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 6) | Ours, decisively for crash-safety and audit; trade-off on depth bound -- see [recommendation 3](#3-do-not-add-a-max_subagent_depth-analog-to-the-proto-schema) |
| `ThreadsDatabase::delete_thread`, stack-based transitive walk over `parent_id`, one mutex, no explicit `BEGIN`/`COMMIT` found | `[ParentTerminated, SessionCancelled]` atomic per-child batch via reconciler, transitive because `SessionCancelled` is itself a terminal marker ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 6) | Ours, decisively -- see [the two gaps the industry has not closed](#the-two-gaps-the-industry-has-not-closed) |
| Copy-to-clipboard "fork" (`copy_thread_to_clipboard`/`load_thread_from_clipboard`), `to_db_thread` resets `subagent_context: None`, no lineage field anywhere | `SessionForked{source_session_id, context_prefix_boundary: SessionOrdinal, reason}`, atomic `[SessionStarted, SessionForked]` batch | Ours, decisively |
| `acp_thread::UserMessage.checkpoint: Option<Checkpoint>` wrapping a `GitStoreCheckpoint` (git commit sha), client-side rendering type only, not persisted to either database | `Checkpoint{reference, checkpoint_type, digest, implementation_version, checkpoint_id, producing_execution_attempt_id, covers_through: SessionOrdinal, session_execution_plan_digest, capture_attestation_ref, capture_attestation_digest, effective_history_digest}`, embedded in `CheckpointProduced`/`ExecutionAttemptStarted.restored_checkpoint`, digest-verified and durable | Ours, decisively -- different "checkpoint" concepts, see [Semantic mismatches](#semantic-mismatches) |
| `sqlez::Domain` migration ratchet on `ThreadMetadataDb` (stored SQL text, hard-fail on drift) | `LEGACY_REQUIRED` field presence + `reserved` retired field numbers on the envelope/event registration (e.g. `execution_attempt_started.proto:29-30`, `reserved 7; reserved resume_cursor;`) | Trade-off, currently a convention on our side, not an enforced ratchet -- see [recommendation 2](#2-add-an-automated-drift-ratchet-for-the-sessionevent-envelope-and-oneof-registration) |
| `ThreadsDatabase`'s swallowed-error `ALTER TABLE ADD COLUMN`, `#[serde(default)]` fields, `DbThread::VERSION` sniffing plus `upgrade_from_agent_1` legacy bridge | Additive optional fields, no per-event version branch ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 3) | Ours is stricter at the boundary (validated, typed, rejected on malformed shape) but has not yet proven it tolerates an *old reader* against a *newer* log the way this axis implicitly tests -- see [recommendation 4](#4-add-forward-compatibility-regression-tests-for-additive-only-replay) |
| `PathList`, set-based path identity (`path_list.rs:27-39`), live-only relocation reconciliation (`sidebar.rs:992-1012`, `agent_panel.rs:4113`), admitted no offline-reconciliation path found | `WorkspaceRef{workspace_id, uri, revision}` on `SessionStarted.workspace`; `workspace_id` is "assigned by the platform and independent of location" (`workspace.proto:14`), binding immutable for session life | Ours, decisively -- identity is decoupled from location structurally, not reconciled after the fact; see [Open questions](#open-questions-for-the-adr) for the residual question this still leaves |
| `RemoteConnectionIdentity`, normalized SSH/WSL/Docker host/user/port matching, scopes which local threads a remote project shows | No equivalent; multi-host/multi-writer correctness is a substrate property (any JetStream replica may append) rather than a client-side scoping filter | Trade-off -- different problems, not comparable; see [Trade-offs](#trade-offs-not-gaps) |
| `ArchivedGitWorktree{worktree_path, main_repo_path, staged_commit_hash, unstaged_commit_hash, original_commit_hash}` | No equivalent; `SessionArchived`/`SessionUnarchived` are pure reversible listing-visibility facts | Gap, deliberate -- git-worktree space reclamation is a workspace-lifecycle concern, not a session-log concern; see [Open questions](#open-questions-for-the-adr) |
| Release-channel isolation (`{db_dir}/0-{scope_name}/db.sqlite`), `ZED_STATELESS` in-memory fallback | No equivalent concept | N/A -- a per-deployment/local-install concern, not a per-session one |
| `channels_with_threads`/`import_threads_from_other_channels`, cross-release-channel self-import | No equivalent | N/A -- Zed's answer to having four separate on-disk copies of its own store, a problem our single-deployment topology does not have |
| `fuzzy_match_positions` title-only in-memory search over the resident `ThreadMetadata` cache; no FTS/vector index found | "Any full-text or vector search subsystem is a separate, independently bootstrapped projection off the same log, out of scope here" ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 8) | Convergent gap -- neither side has built this yet |
| `DbThread.cumulative_token_usage`, `request_token_usage: HashMap<ClientUserMessageId, TokenUsage>` | `TokenUsage` on `CanonicalMessage.usage` and `Compacted.usage`, folded per read model | Ours -- no denormalized cumulative counter stored in the row to drift from the log |
| `DbThread.model`, `profile`, `speed`, `thinking_enabled`, `thinking_effort` as mutable per-thread fields, no change event found | `ModelSettings{max_output_tokens, temperature, top_p, thinking_budget_tokens, stop_sequences, raw_settings}` on `AssistantMessageStarted.settings`, per-completion | Ours -- a mid-session settings change is two adjacent facts, not a field overwrite the next document save silently carries |
| `DbThread.draft_prompt: Option<Vec<acp::ContentBlock>>` (unsent draft text) | No equivalent | Gap, deliberate -- an unsent draft is not a "happened" fact; recording client-local unsent input in the durable log would leak view state into the aggregate |
| `DbThread.ui_scroll_position`, `sandboxed_terminal_temp_dir`, `sandbox_grants` | No equivalent | Gap, deliberate -- UI/view state and sandbox runtime plumbing, not domain facts of the run |

## Semantic mismatches

**"Session id" is a wire identity in Zed and a pure storage key in ours -- and
Zed pays for the conflation with a second keyspace.** `ThreadMetadata.session_id:
Option<acp::SessionId>` (`crates/agent_ui/src/thread_metadata_store.rs:311`)
and `ThreadsDatabase::load_thread`/`delete_thread`
(`crates/agent/src/db.rs:607,671`) are keyed by `acp::SessionId` -- the literal
Agent Client Protocol wire type, not a storage-internal id Zed minted for its
own purposes. Content, by contrast, is Zed's own internal `Message`/
`UserMessageContent`/`AgentMessageContent` types, converted to and from the ACP
wire schema only at two named boundary functions
(`UserMessageContent::from_content_block`, `thread.rs:6717`; `impl From<UserMessageContent>
for acp::ContentBlock`, `thread.rs:6773`; the dossier's [Entry/message structure and versioning](./index.md#entrymessage-structure-and-versioning)). So identity
borrows a wire protocol's type directly as a database primary key, while
content gets a translation boundary; the asymmetry is exactly backwards from
where instability actually lives; a client-facing wire protocol's identity
type is exactly the kind of thing more likely to gain a v2 (the sibling ACP
corpus already documents `agent-client-protocol-schema` mid-migration to a
`v2.0.0-alpha.2` JSON Schema line, [docs/research/acp/products/zed.md:9](../../../acp/products/zed.md)),
while message content shape is comparatively more stable.

Our `SessionId` is opaque and minted by us, independent of any wire protocol we
also happen to speak ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 1/2: "a `StreamSubjectResolver` maps the
opaque `SessionId` to the subject... `SessionId` is opaque and time-sortable by
construction... but its sort order is never load-bearing"). What opaque
identity buys that Zed's ACP-as-primary-key choice gives up: an ACP schema
version bump can never force a storage-key migration on us, because our
storage key was never the wire type in the first place. What Zed pays for the
conflation concretely: a second identity (`ThreadId`, a newtype `uuid::Uuid`,
`thread_metadata_store.rs:34`) has to exist *only* because a metadata row can
predate a session (a draft), plus an in-memory reverse index
`threads_by_session: HashMap<acp::SessionId, ThreadId>`
(`thread_metadata_store.rs:505`) that must stay consistent with two
independently-keyed, independently-migrated on-disk tables -- a second
consistency surface our design does not have, because nothing in our catalog
exists before `SessionStarted` (there is no "pre-session" phase in the store;
[recommendation 1](#1-do-not-introduce-a-pre-session-draft-identity-or-second-keyspace)
makes this explicit as a rejection of a plausible future feature request).

**"Checkpoint" names three unrelated things, two of them inside Zed itself.**
Zed's `Checkpoint` (wrapping `GitStoreCheckpoint`/`GitRepositoryCheckpoint{commit_sha:
Oid}`, `crates/project/src/git_store.rs:321`, `crates/git/src/repository.rs:1278`)
is a git commit sha of a WIP stash-like commit, used to rewind *file state* to
a point in a live session; it lives only on the ephemeral, in-memory
`acp_thread::UserMessage` client-rendering type and is never written to either
SQLite database (the dossier's [Rewind, checkpoints, and fork](./index.md#rewind-checkpoints-and-fork) -- "we found no corresponding
`checkpoint` field on the durable `agent::thread::UserMessage` / `Message::User`
type"). Our `Checkpoint`
(`proto/trogonai/session/sessions/v1alpha1/checkpoint.proto:17`) is a harness
recovery checkpoint: a digest-verified, out-of-line artifact reference with its
own `checkpoint_id`, `producing_execution_attempt_id`, and
`session_execution_plan_digest`, embedded durably in `CheckpointProduced` and
`ExecutionAttemptStarted.restored_checkpoint` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 3). These are not
approximately the same thing wearing different names -- Zed's checkpoint
restores workspace files for a UI undo action and vanishes on restart; ours
restores harness process state and is the one artifact [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)'s "Four
records with separate authority" section goes out of its way to distinguish
from the event log, the aggregate snapshot, and a read-side checkpoint. A
reader who assumes "Zed has checkpoints too" implies durability would be
wrong twice over: Zed's is not durable, and even if it were, it answers a
different question than ours does.

## What we should consider changing

Ordered by how consequential it is to get wrong, not by implementation cost.

### 1. Do not introduce a pre-session "draft" identity or second keyspace

**The change (rejected):** do not add a metadata row, a "pending session," or
any identity that can exist before `SessionStarted` is appended. Keep
`SessionId` as the sole key from creation, per [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2 ("opaque
addressing key") and the `CreateSession` command's `NoStream` precondition
(command matrix, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) §"Command-by-command matrix": `CreateSession` reads
no state and requires the stream not exist).

**Evidence anchor:** Zed (store maturity 11/12).
`ThreadMetadata.session_id: Option<acp::SessionId>`
(`crates/agent_ui/src/thread_metadata_store.rs:311`), comment "Drafts may not
have a session_id yet; only index by session" (the dossier's [Keying and identity](./index.md#keying-and-identity)), and
the in-memory reverse index `threads_by_session: HashMap<acp::SessionId,
ThreadId>` (`:505`) it takes to keep the two keyspaces reconciled.

**Blast radius:** breaking the decision, not the schema -- this is a
rejection of a plausible future ask (a product wants "compose a message
before a session exists" UX, exactly Zed's draft feature), not a proposal to
change today's proto.

**Why it is not a good idea:** Zed's own dossier shows the cost directly --
`ThreadId` exists *only* to key drafts, `ThreadMetadataStore` maintains a
`threads_by_session` map purely to translate between the two, and the store
has two independently-migrated tables (`sqlez` ratchet vs. ad hoc `ALTER
TABLE`) whose consistency the reverse index has to bridge at runtime. None of
this is a defect in Zed's design for its purpose (an editor needs to let a
user type before committing to an agent run); it is the cost of letting
"session" mean two different lifecycle stages. Our aggregate has exactly one
lifecycle stage: a stream that does not exist, and a stream that does. A
client wanting draft-composition UX can hold unsent text locally and call
`CreateSession` only once a first message is ready -- the store never needs to
model the in-between state.

**What it costs to reject this:** none directly; the cost is opportunity --
if a product team later wants Zed-style draft persistence (survive an app
restart with unsent text), that has to be solved above the session store
(client-local storage), not inside it. Recording that trade-off here so it is
not re-proposed without this evidence.

### 2. Add an automated drift ratchet for the `SessionEvent` envelope and oneof registration

**The change:** add a golden-file or generated-descriptor comparison test that
fails the build if an already-shipped `SessionEvent` oneof field number,
event-file `reserved` range, or envelope field (`Event.id`, timestamps,
correlation/causation headers) changes incompatibly -- mirroring `sqlez`'s
approach of storing each migration's exact text and diffing it on every boot,
but applied to our proto registration rather than SQL. This is additive
tooling around [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3's existing rule ("Schema evolution is
additive... never a per-event version branch") and the `reserved` numbers
already present, e.g. `proto/trogonai/session/sessions/v1alpha1/execution_attempt_started.proto:29-30`
(`reserved 7; reserved resume_cursor;`).

**Evidence anchor:** Zed (store maturity 11/12, and this is the axis where the
tension noted above cuts both ways). `sqlez::Connection::migrate`
(`crates/sqlez/src/migrations.rs:37-104`) stores every applied migration's
formatted SQL text in a `migrations` table and panics on any subsequent
mismatch unless the domain opts in via `should_allow_migration_change`
(`domain.rs:7-9`), confirmed by its own regression test
`changed_migration_fails` (`crates/sqlez/src/migrations.rs:311-346`). Contrast:
`ThreadsDatabase`'s `ALTER TABLE ... ADD COLUMN` runs with errors swallowed if
the column already exists (`crates/agent/src/db.rs:456-471`) and keeps no
ledger at all -- the exact failure mode a drift ratchet exists to catch never
gets caught there, it just silently no-ops or silently succeeds.

**Blast radius:** additive -- new CI tooling and a golden descriptor snapshot,
no schema change.

**Why it is a good idea:** decision 3's additive-only rule is currently
enforced by review discipline and `reserved` numbers a human has to remember
to add, exactly the situation `ThreadsDatabase` is in today (a convention with
no ledger). `ThreadMetadataDb`'s ratchet is the corpus's best evidence that
the alternative -- a machine-checked, hard-failing comparison against the
prior shipped shape -- is buildable and has already caught real drift in a
shipped product. We should be on that side of the asymmetry the maturity
tension surfaces, not the swallowed-`ALTER-TABLE` side.

**What it costs beyond migration:** a golden descriptor file to maintain and
regenerate deliberately on every accepted additive change (a small, repeated
review step); a new CI failure mode to diagnose when it fires on an
unintentional break; no runtime cost, since this is a build-time check, not a
storage-boundary validator.

### 3. Do not add a `MAX_SUBAGENT_DEPTH` analog to the proto schema

**The change (rejected):** do not add a depth counter or a hard nesting limit
to `DelegationDispatched`, `ParentLinked`, or `CascadePolicy`. [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)
decision 6 already gives acyclicity by construction (a fresh
`child_session_id` every dispatch, `NoStream` rejects re-parenting an
existing stream) without needing to bound depth to get it.

**Evidence anchor:** Zed (store maturity 11/12).
`MAX_SUBAGENT_DEPTH: u8 = 1` (`crates/agent/src/thread.rs:77`), checked at
`thread.rs:2169` (`if self.depth() < MAX_SUBAGENT_DEPTH`) before allowing a
further spawn.

**Blast radius:** additive, if ever pursued -- but the recommendation is to
pursue this only as a command-authorization policy check (draft [ADR#0026](../../../../adr/0026-command-authorization-principal.md)'s
`CommandPrincipal`/`CommandAuthorizer`), never as a proto field, so there is no
schema blast radius at all for the rejected option.

**Why it is not a good idea as a schema change:** acyclicity already prevents
the failure mode a depth bound is usually defending against (infinite
self-reference); an *unbounded but acyclic* tree is a resource/product policy
question (how much fan-out or nesting is acceptable for a given deployment or
tenant), not a store-correctness question, and different deployments may
reasonably want different bounds. Baking `max_depth = 1` into the event
schema the way Zed does would foreclose that per-deployment flexibility and
conflate a policy decision with a durable fact -- the exact anti-pattern
`CascadePolicy` (D6) already avoids by making cascade behavior data instead of
code.

**What it costs to reject this:** an unbounded delegation chain is possible
today until a policy-layer bound exists; this is recorded as an
[open question for the ADR](#open-questions-for-the-adr) rather than silently
assumed away.

### 4. Add forward-compatibility regression tests for additive-only replay

**The change:** add tests asserting that an older reader of the codec/decoder
(one built against an earlier accepted set of optional fields) correctly
ignores unknown/newer optional fields on replay rather than failing closed,
for every event type under `validate_session_event` and the Session-owned
replay boundary ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 3).

**Evidence anchor:** Zed (store maturity 11/12, this specific finding drawn
from its own unresolved Open Question, so it is thin evidence for the
*problem* rather than a solution -- flagged accordingly). The dossier
could not find explicit handling for "what happens when an **old** Zed binary
opens a database that a **newer** Zed binary has already migrated forward" --
the ratchet only checks migrations the old binary's compiled `MIGRATIONS`
array knows about, with no ceiling check for later steps it does not know
about at all (dossier Open Questions, [Open questions](./index.md#open-questions)).

**Blast radius:** additive -- test-only, no schema or validator change assuming
the current additive-only discipline already holds; the test's purpose is to
prove that assumption rather than change behavior.

**Why it is a good idea:** decision 3's promise ("Schema evolution is
additive... never a per-event version branch") is currently unverified by an
automated test in the direction that actually matters operationally: an
older running service instance processing a stream a newer instance already
wrote to. Protobuf's wire format tolerates unknown fields for free, so this
may already work -- but "may already work" is exactly the gap Zed's own
unresolved question shows a mature, shipped store can still leave open for
years. Proving it with a fixture-based test (encode with a newer field set,
decode with an older generated-code snapshot, assert no rejection) turns an
assumption into a checked invariant.

**What it costs beyond migration:** a small fixture corpus of "prior accepted
shape" golden messages per event type, regenerated deliberately whenever a
new optional field is accepted (a similar discipline to recommendation 2's
golden descriptor, and plausibly the same CI job).

## Trade-offs, not gaps

**Synchronous mutex-held cascade delete versus eventually-consistent saga
cascade.** Zed's `ThreadsDatabase::delete_thread` holds one
`Mutex<Connection>` for the entire stack-based walk-and-delete of a parent and
all its transitive children (`crates/agent/src/db.rs:671-716`) -- cascade is immediate and,
modulo the missing explicit `BEGIN`/`COMMIT` (see
[the two gaps the industry has not closed](#the-two-gaps-the-industry-has-not-closed)), effectively atomic from the caller's
perspective. Our reconciler cascades via a `[ParentTerminated, SessionCancelled]`
atomic batch **per child**, discovered through a parent-to-children lineage
projection, taking "D sequential reconciler round-trips" for a chain of depth
D ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 6 Consequences). Zed buys immediacy and a single lock scope
at the cost of a single-process, single-database assumption that cannot
survive a cross-stream write (which does not exist on JetStream, and which
our own Alternatives Considered rejects for exactly that reason). We buy
crash-safety, multi-writer correctness, and an audit trail that survives
cascade (nothing is deleted, only marked `SessionCancelled`) at the cost of
latency and eventual -- not immediate -- consistency. Neither is wrong; they
are solving different constraint sets (single embedded SQLite file vs. a
distributed append-only log with no cross-subject transaction).

**Hard depth cap versus acyclic-but-unbounded tree.** Covered in
[recommendation 3](#3-do-not-add-a-max_subagent_depth-analog-to-the-proto-schema)
as a rejected schema change, but it is also a genuine trade-off independent of
that recommendation: Zed's `MAX_SUBAGENT_DEPTH = 1` buys a predictable,
reviewable resource bound at the cost of rejecting legitimate deeper
collaboration patterns outright; our design buys flexibility (any depth, so
long as it is acyclic) at the cost of no built-in guard against runaway
fan-out, which is why it is called out as an open question rather than
silently accepted.

**Ratchet-with-hard-fail versus reserved-numbers-by-convention.** `sqlez`'s
migration ratchet (`crates/sqlez/src/migrations.rs:37-104`) buys Zed a
hard-fail on any drift to `ThreadMetadataDb`'s schema, at the cost of a real
operational failure mode: an edited migration string is a startup crash for
every user, not a warning. Our envelope/oneof schema currently buys a softer
failure mode (a reviewer has to notice a `reserved` number should have been
added) at the cost of no automated enforcement yet -- which is not a settled
trade-off so much as an open gap, addressed by
[recommendation 2](#2-add-an-automated-drift-ratchet-for-the-sessionevent-envelope-and-oneof-registration).

## What our design already does better

**Rewind is an appended fact, not an in-memory `Vec::drain`.**
`SessionRewound.keep_through` (`proto/trogonai/session/sessions/v1alpha1/session_rewound.proto:18`,
a `SessionOrdinal`, inclusive) leaves every event on the stream; only the
model-visible fold changes. Zed's `Thread::truncate`
(`crates/agent/src/thread.rs:2359-2383`) does `self.messages.drain(position..)`
and the next reactive save permanently erases the tail from `threads.db` -- "no
tombstone, no soft-delete, and no server-side ability to un-rewind once a save
has landed" (the dossier's [Rewind, checkpoints, and fork](./index.md#rewind-checkpoints-and-fork)). This is the clearest, most concrete
win in the comparison: the same operation is recoverable-by-construction on
our side and permanently destructive on theirs.

**Fork is atomic and carries real lineage; Zed's is copy-and-forget.**
`SessionForked{source_session_id, context_prefix_boundary, reason}`
(`proto/trogonai/session/sessions/v1alpha1/session_forked.proto:17`) is the
second event in an atomic `[SessionStarted, SessionForked]` batch under
`NoStream` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 5). Zed's only fork-like feature is
`copy_thread_to_clipboard`/`load_thread_from_clipboard`
(`crates/agent_ui/src/agent_panel.rs:3717,3777`), whose `to_db_thread`
conversion explicitly resets `subagent_context: None` and every other
identity-adjacent field to default, mints a brand-new `acp::SessionId`, and
records "no parent/lineage pointer, shared-prefix reference, or origin session
id... anywhere" (the dossier's [Rewind, checkpoints, and fork](./index.md#rewind-checkpoints-and-fork)). A forked session in our catalog
can always answer "where did you come from"; a pasted thread in Zed cannot.

**Harness recovery checkpoints are durable and digest-verified; Zed's file-state
checkpoint does not survive a restart.** Our `Checkpoint`
(`proto/trogonai/session/sessions/v1alpha1/checkpoint.proto:17`) is embedded in
`CheckpointProduced`/`ExecutionAttemptStarted.restored_checkpoint`, carries its
own `checkpoint_id`, `covers_through`, and `session_execution_plan_digest`, and
is admitted only after full digest/plan/attempt verification ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet
3). Zed's `acp_thread::UserMessage.checkpoint: Option<Checkpoint>`
(`crates/acp_thread/src/acp_thread.rs:294-307`) wraps a git commit sha and lives
only on an ephemeral client-rendering type never persisted to either database
-- "reopen the thread after a restart and the file-state rewind capability for
past turns is gone even though the message text remains" (dossier
[Rewind, checkpoints, and fork](./index.md#rewind-checkpoints-and-fork)).

**Cascade is a crash-safe, two-fact saga; Zed's two-database delete has an
admitted inconsistency window.** `DelegationDetached`/`ParentDetached`
(`proto/trogonai/session/sessions/v1alpha1/delegation_detached.proto`,
`parent_detached.proto`) are joined by one durable `detach_operation_id`, each
its own invariant-bearing local fact, with idempotent reconciler repair on
either side ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 6). Zed's `ThreadMetadataStore::delete` and
`ThreadsDatabase::delete_thread` are "two separate delete calls against two
separate databases -- we found no single transactional operation spanning both
`db.sqlite` and `threads.db`, so a crash between the two delete calls could
leave one store's row present without its counterpart" (dossier
[Retention, deletion, and multi-host](./index.md#retention-deletion-and-multi-host), also listed under its own Open Questions).

**Cascade policy is a recorded choice, not a hardcoded behavior.**
`CascadePolicy` (`proto/trogonai/session/sessions/v1alpha1/cascade_policy.proto:8`)
is a two-arm enum -- `CASCADE_ON_PARENT_TERMINAL` or `INDEPENDENT` -- chosen at
dispatch time and copied verbatim onto the child, so the answer to "what
happens to this child when its parent dies" is data on the log, auditable per
child. Zed's cascade is unconditional: every subagent is deleted when its
parent is (`crates/agent/src/db.rs:671-716`), a single hardcoded behavior with no equivalent to
`INDEPENDENT`.

**Workspace identity is decoupled from location by construction, not
reconciled after the fact.** `WorkspaceRef.workspace_id`
(`proto/trogonai/session/sessions/v1alpha1/workspace.proto:14-15`) is "assigned
by the platform and independent of location," carried inline on
`SessionStarted` so "every session for this workspace" never requires decoding
plan bytes. Zed's `PathList` compares only the literal set of absolute paths
(`crates/util/src/path_list.rs:27-39`); a rename or move mints a new identity
key, and reconciliation is a live event subscription bolted on at the
workspace layer (`sidebar.rs:992-1012`) that the dossier could not confirm
fires for a rename that happened while Zed was not running ([Keying and identity](./index.md#keying-and-identity)).

**Fact-granularity commit means no reactive whole-document rewrite
bottleneck and no lost in-flight turn.** Covered fully under
[the one structural difference](#the-one-structural-difference-everything-else-follows-from);
restated here because it is squarely a place our design is ahead, not merely
different -- Zed's own dossier independently concludes the same thing ("Zed is
a mutable-document store dressed in ACP's identity vocabulary... not an
event-sourced system," [What this implies for our Session Store](./index.md#what-this-implies-for-our-session-store-our-inference)).

## What not to copy

Zed is licensed **per-crate**. Per the dossier's front matter, every crate
cited in this document is `GPL-3.0-or-later` **except** `gpui`, `util`
(which is where `path_list.rs` lives), and `collections`, which are
`Apache-2.0`. Concretely: `crates/agent`, `crates/agent_ui`, `crates/db`,
`crates/sqlez`, `crates/remote`, `crates/project`, `crates/git`, and
`crates/acp_thread` are all `GPL-3.0-or-later`. Every pattern below that is
worth learning *from* as an architectural idea is not safe to copy as literal
code into our codebase, because the code text itself carries that license;
only `PathList`'s own implementation (`crates/util/src/path_list.rs`,
Apache-2.0) would be license-safe to port verbatim, and this document does not
recommend porting it (see the identity-decoupling point above -- our
`workspace_id` design is already ahead of `PathList`'s approach).

- **Reactive whole-document overwrite on every observable mutation.**
  `cx.observe(&thread_handle, ...)` → `save_thread` on essentially every GPUI
  change notification (`crates/agent/src/agent.rs:820,1736`) is the direct
  cause of destructive rewind and the absence of any OCC precondition. Do not
  adopt "just re-save the whole aggregate on every change" as a pattern
  anywhere in the platform, even for a small or simple aggregate -- it
  silently forecloses partial resume and non-destructive undo the moment
  anyone builds a feature (like rewind) that needs them.
- **Swallowed-error `ALTER TABLE ... ADD COLUMN` with no migration ledger.**
  `crates/agent/src/db.rs:456-471` treats a failed column-add as
  indistinguishable from "already applied." Silent schema drift is
  undetectable by definition; this is the opposite of
  [recommendation 2](#2-add-an-automated-drift-ratchet-for-the-sessionevent-envelope-and-oneof-registration)'s
  direction and should never be the model for how our own additive evolution
  is enforced.
- **Cross-store delete without a shared transaction or a joining saga id.**
  `ThreadMetadataStore::delete`/`ThreadsDatabase::delete_thread` are two
  independent calls against two independent SQLite files with no
  cross-database transaction (the dossier's [Retention, deletion, and multi-host](./index.md#retention-deletion-and-multi-host)). If we ever have a
  legitimate reason to split a session's data across two independently-owned
  stores, the answer is [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 6's two-fact saga pattern
  (`DelegationDetached`/`ParentDetached` joined by `detach_operation_id`), not
  an unguarded double-delete with an admitted inconsistency window.
- **Clipboard-copy "fork" with identity reset and zero lineage.**
  `to_db_thread` resetting `subagent_context: None` and minting a disconnected
  `acp::SessionId` with no origin reference (the dossier's [Rewind, checkpoints, and fork](./index.md#rewind-checkpoints-and-fork)) is not
  a lightweight-fork pattern worth any version of adopting, even for a
  deliberately minimal "duplicate this session" feature -- the missing
  `source_session_id` is the entire value a fork provides over a plain copy.

## The two gaps the industry has not closed

### Subagent cascade

Our position is already decided: [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6 -- parent-first dispatch
with crash-safe repair, acyclicity by construction, rewind invalidation kept
distinct from terminal cascade, transitive cascade via a reconciler process
manager, and a two-fact detach saga joined by `detach_operation_id`. Zed's
evidence bears on that decision directly rather than leaving it unaddressed.

**Zed validates transitive cascade-on-terminal as the right default.**
`ThreadsDatabase::delete_thread`'s stack-based walk over `parent_id`
(`crates/agent/src/db.rs:671-716`) genuinely finds and deletes every
transitive descendant, not just direct children -- "a clear point of contrast
with stores that orphan child sessions on parent deletion" (dossier
[Subagents and nested sessions](./index.md#subagents-and-nested-sessions)). This matters because Zed is the exception in the wider
corpus, not the rule: [synthesis.md](../../synthesis.md) convergence #7 records
that Codex CLI, Goose, OpenCode, and T3 Code all *orphan* subagents on parent
delete rather than cascading, and the one other product studied with an
apparent cascade guarantee, Cline, has it "only... one level deep, and only
from a root" -- its child-delete query "sits inside `if (!row.isSubagent)`," so
"deleting a session that is itself a subagent never looks for its own
children," making the guarantee "a property of how deep the graph happens to
get, not of the delete algorithm"
([Cline, Subagents and nested sessions](../cline/index.md#subagents-and-nested-sessions)).
Zed's walk has no such limitation -- it is genuinely recursive over `parent_id`
regardless of depth. Decision 6's choice to make cascade "transitive, because
`SessionCancelled` is itself a terminal marker the same reconciler reacts to"
is therefore not a hypothetical improvement over an unproven industry
practice; it is validated by the one product in the corpus that actually
built transitive cascade and chose the same behavior Zed did, independently.

**Where decision 6 is already ahead of Zed's implementation of the same
choice.** Zed's cascade walk runs "under one held `Mutex<Connection>` guard
but we did not find it wrapped in an explicit SQL `BEGIN`/`COMMIT`" (dossier
[Subagents and nested sessions](./index.md#subagents-and-nested-sessions), also an Open Question) -- a crash mid-walk could leave a
partially-deleted subtree with no recorded state to resume from, and because
delete is physical, there is nothing to retry against once some rows are
gone. Our reconciler's `[ParentTerminated, SessionCancelled]` batch per child
is individually atomic and idempotently retryable (the command matrix's
`ReconcileParentTerminal` row: "no-op if child already terminal"), so a crash
mid-cascade at any depth resumes cleanly rather than leaving a structurally
ambiguous partial state. Zed's cascade also permanently deletes; ours marks
`SessionCancelled` on a keep-forever log (decision 7), so the entire
collaboration tree remains auditable *after* cascade runs, which Zed's design
cannot offer once a delete has executed.

**Where Zed has nothing corresponding to decision 6's rewind/termination
split.** The dossier explicitly "found no code handling parent rewind/crash
propagating to a still-live subagent (e.g. cancellation) -- only the
delete-cascade path was confirmed" ([Subagents and nested sessions](./index.md#subagents-and-nested-sessions), Open Questions). Zed --
the one product in the corpus with a real cascade -- still has no analog to
`ParentHistoryInvalidated`/`SessionRewound.keep_through` invalidating a
child whose dispatch point no longer exists in the parent's history, while
the parent itself keeps running (not terminal). Decision 6's insistence that
"a rewound parent is not terminal and may keep running, [so] invalidating
such a child is not the same event as a terminal cascade" is therefore a
genuine capability gap Zed's cascade does not close, reinforcing that this
part of decision 6 is original design work, not industry-standard practice
restated.

**Depth bound remains a refinement question, not a gap in decision 6.**
Decision 6 gives acyclicity, never a depth bound; Zed's hard
`MAX_SUBAGENT_DEPTH = 1` (`thread.rs:77`) is evidence that a real shipped
product found value in bounding depth, but as
[recommendation 3](#3-do-not-add-a-max_subagent_depth-analog-to-the-proto-schema)
argues, that bound belongs at a policy layer, not the event schema -- recorded
as an [open question](#open-questions-for-the-adr), not treated as a defect in
decision 6.

### Retention on an unbounded log

Our position is already decided: [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7 -- keep-forever,
`SessionHidden` replacing `SessionDeleted` as an honest visibility tombstone,
`RedactionApplied` for read-time masking of specific event ids, `ArtifactErased`
for out-of-band artifact-byte destruction independent of event-log retention,
and aggregate snapshots that bound replay cost, not storage.

Zed contributes little new evidence here, and the reason is itself
informative: a mutable-document store never actually faces the failure mode
decision 7 exists to solve. The dossier found "no lifecycle policy or
scheduled-cleanup mechanism... for ordinary threads -- deletion is user-driven
(sidebar delete action) or explicit archival, not time-based expiry" ([Retention, deletion, and multi-host](./index.md#retention-deletion-and-multi-host)).
The one retention-adjacent feature, `ArchivedGitWorktree`
(`thread_metadata_store.rs:457-470`), reclaims **git-worktree disk space** tied
to thread archival -- a workspace-lifecycle feature, not a session-log
retention story at all. Zed has no analog whatsoever to `SessionHidden`,
`RedactionApplied`, or `ArtifactErased`: its "archive" (`archive`/`unarchive`,
`thread_metadata_store.rs:858,873`) is fully reversible and carries none of
`SessionHidden`'s terminal-marker/cascade semantics (compare
`session_hidden.proto:13` -- a typed, terminal `SessionHiddenReason` that
still cascades per decision 6 -- against Zed's archive, which is closer in
shape to our own reversible `SessionArchived`/`SessionUnarchived`). Zed never
built partial masking of an otherwise-immutable record because its record is
never immutable in the first place -- a whole document is simply replaced or
deleted, which is a strictly easier problem than masking *part of* a
keep-forever fact stream while leaving the rest intact.

This is not a criticism of Zed; it is the structural consequence of the "one
difference" identified above. It does mean this specific product cannot serve
as evidence either for or against decision 7's specific mechanisms
(`SessionHidden`/`RedactionApplied`/`ArtifactErased`) the way an actual
log-shaped store could -- Zed simply never had to design for this problem.
The one genuinely new datum it contributes is `ArchivedGitWorktree`'s
space-reclamation angle, which points at a real but different problem
(reclaiming space bound to a *workspace*, not a session log) that decision 7
does not claim to cover and, per its scope, should not -- recorded as an
[open question](#open-questions-for-the-adr) rather than folded silently into
decision 7's boundary.

## Open questions for the ADR

- Should draft [ADR#0026](../../../../adr/0026-command-authorization-principal.md)'s `CommandAuthorizer` carry a configurable
  maximum-delegation-depth policy check, layered above decision 6's
  acyclicity-by-construction, given Zed's `MAX_SUBAGENT_DEPTH = 1` is real
  shipped evidence that *some* products want a hard bound even though decision
  6 never claimed to need one for correctness?
- Is an automated drift ratchet for the `SessionEvent` envelope/oneof
  registration (recommendation 2) worth building before `v1alpha1` promotes to
  `v1`, given `sqlez`'s hard-fail ratchet is the corpus's best evidence that
  such a mechanism is buildable and has caught real drift in a shipped
  product, while our own additive-only rule is currently convention-enforced
  only?
- Is workspace-identity resolution (mapping a stable `WorkspaceRef.workspace_id`
  back to its possibly-relocated `uri`) ever a session-store concern, or is it
  entirely external, given `WorkspaceRef` is immutable for a session's life
  and a location change requires a new session or fork? Zed's live-only path
  reconciliation and admitted no-offline-reconciliation gap (dossier Open
  Questions, [Open questions](./index.md#open-questions)) is a caution about what happens when this kind
  of concern is left informally bolted on elsewhere rather than answered once,
  explicitly.
- Does decision 7's `ArtifactErased` or the optional cold-tiering job ever need
  to reach into workspace-adjacent storage reclamation -- the problem Zed's
  `ArchivedGitWorktree` solves at the workspace layer -- or is that
  unambiguously a different aggregate's concern, out of [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)'s scope
  entirely?

## Things this document could not verify

- Zed's own dossier flags several claims as unread-in-full or inferred rather
  than confirmed (the exact pre-`0.3.0` legacy schema shape, the
  `open_fallback_db` trigger condition, whether an offline worktree-rename
  reconciliation path exists, the exact `acp::SessionId` minting call site).
  Every citation in this document to those specific claims carries the same
  uncertainty the stage-one dossier already recorded; none of it is restated
  here as more certain than the dossier states it.
- This document did not independently re-read the cited Zed source lines
  against a fresh checkout; it relies on the stage-one dossier's pinned
  `path:line` anchors at commit `4aad57fd1f002f9feeea2b7fb6229ccbcd576cb1`, per
  this prompt's precondition that a verified dossier is trustworthy input.
