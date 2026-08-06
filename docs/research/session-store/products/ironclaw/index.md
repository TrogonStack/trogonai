# IronClaw: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-08-05. Version-sensitive claims were checked
against these authoritative anchors:

- Repository `github.com/nearai/ironclaw` at commit
  `2ae66212fe80208524179047878916dafc0538ee` (committed 2026-08-05T18:20:35Z,
  "feat(product): add new, stop, and interrupt commands (#6969)"). Rust
  workspace, dual MIT OR Apache-2.0.
- `crates/domains/ironclaw_threads/src/{service.rs,contract.rs,filesystem_service.rs,stored_message.rs,summary_artifacts.rs}`
  for the transcript boundary.
- `crates/substrates/ironclaw_filesystem/src/{root.rs,record.rs}` for the
  durable substrate the transcript is written through.
- `crates/Architecture.md` and `docs/reborn/contracts/{turn-persistence.md,conversation-binding.md,storage-placement.md,agent-loop-protocol.md}`
  for the architectural contracts.
- `docs/reborn/subagent-spawn/README.md` for child-session storage.
- `migrations/V1..V34` for the SQL shape of the backing store.

Citations use repo-relative `path:line` shorthand against that commit.

> Scope note. IronClaw has no published product documentation site; every
> primary source below is in-repository (design contracts under `docs/reborn/`
> plus the implementing Rust code). The tree is mid-transition from a legacy
> pre-"Reborn" design to the Reborn architecture, and the two disagree in
> places. Where they do, `crates/Architecture.md` and the code win: the repo
> states this rule explicitly at
> `docs/reborn/target-architecture/CHECKLIST.md:355` ("**THE CODE WINS**").
> This dossier documents the Reborn transcript boundary and flags legacy
> survivals where they are still reachable.
>
> Naming trap, called out by the repo itself: there are *two* types named
> `SessionThreadService`. The transcript one lives in `ironclaw_threads`; the
> inbound-routing one in `ironclaw_conversations` was renamed
> `InboundConversationService` precisely to end the collision
> (`docs/reborn/contracts/conversation-binding.md`). Everything below is the
> `ironclaw_threads` one unless stated.

## The storage model

IronClaw splits durable session state across **two** boundaries with different
owners, and the split is the most load-bearing fact about its model.

1. **The transcript boundary** (`SessionThreadService`): threads, messages,
   and summary artifacts. Source of truth for what was said.
2. **The turn/process boundary** (`ProcessJournalStore`, projected as
   `turn_runs` and friends): run lifecycle, leases, checkpoints, admission
   reservations, idempotency outcomes. Source of truth for what ran.

`docs/reborn/contracts/turn-persistence.md:22` draws the line in one sentence:

> It does **not** own canonical transcript/message storage. Transcript and
> thread-message history remain in the transcript/thread storage boundary.

And the reverse direction is enforced as a redaction rule rather than a
convention (`turn-persistence.md:106-108`):

> Turn persistence stores metadata and references only. It must not persist raw
> prompts, assistant content, tool input, secrets, host paths, or backend error
> details in turn/run/checkpoint/idempotency records.

Within the transcript boundary, the durable session is **a set of individually
CAS-versioned records at virtual paths**, not an append-only log and not one
mutable document. The record layout is stated verbatim in the module doc of the
production implementation (`filesystem_service.rs:22-31`):

```text
/threads[/agents/<agent>][/projects/<project>][/owners/<owner_user>][/missions/<mission>]/threads/<thread_id>/thread.json
/threads[/.../...]/threads/<thread_id>/messages/<message_id>.json
/threads[/.../...]/threads/<thread_id>/summaries/<summary_id>.json
/threads/idempotency/<sha256>.json
```

Three further paths exist under the same thread root
(`filesystem_service.rs:2918-2970`): `messages/` (directory), `tool_results/
sha256-<hex>.bin` for out-of-band tool payloads, and `message_sequence` for the
native per-thread counter row.

That "virtual filesystem" is not necessarily a filesystem. It is the
`RootFilesystem` substrate, whose canonical inhabitant is a SQL row. Every
entry is either an opaque byte file or a typed record
(`crates/substrates/ironclaw_filesystem/src/record.rs:258-279`):

> - **Opaque file**: `body` carries arbitrary bytes, `kind` is `None`,
>   `indexed` is empty. [...]
> - **Record**: `body` carries the serialized payload (typically JSON), `kind`
>   names the schema family [...] and `indexed` declares the projection that
>   backends should expose to [`query`].
>
> Backends never look inside `body` for indexing; everything queryable lives in
> `indexed`.

The SQL migration that introduced this shape shows the physical form
(`migrations/V28__root_filesystem_records.sql:1-19`): `root_filesystem_entries`
gains `content_type`, `kind`, `indexed JSONB`, and `version BIGINT`, where
"`version` enables compare-and-swap semantics on `put`". Thread records declare
four kinds (`filesystem_service.rs:101-104`): `session_thread`,
`thread_message`, `thread_summary`, `thread_idempotency`. Setting `kind` is
load-bearing for safety, not just typing (`filesystem_service.rs`, module doc):

> Setting `entry.kind` makes writes record-shaped so `DiskFilesystem` [...]
> triggers the fail-closed path on the CAS gate instead of accepting a
> byte-only first write without CAS enforcement.

**Authoritative versus derived.** The per-record JSON entries are
authoritative. Everything that makes listing and range reads cheap is an
explicitly rebuildable projection: the thread index rows
(`filesystem_service/thread_index.rs`), the ordered message index, and the
exact-lookup projections, all rebuilt idempotently by a one-time migration pass
(`filesystem_service/transcript_migration.rs:1-60`). The storage-placement
contract states the general rule
(`docs/reborn/contracts/storage-placement.md`): "these views are projections,
not the source of truth". An in-process one-shot context-window cache
(`ONE_SHOT_CONTEXT_WINDOW_CACHE_MAX_ENTRIES: usize = 4096`,
`filesystem_service.rs:110`) is pure cache, seeded on first accept and
invalidated on any later write.

**Conceptual model.** Session-as-directory-of-records, where the "directory" is
a scoped virtual path tree that is usually SQL rows, and each record carries its
own version token. It is neither session-as-log (no append-only event stream is
the source of truth for the transcript) nor session-as-document (no whole-thread
rewrite) nor session-as-row (the thread record is a small metadata row; the
messages are siblings, not columns). `RootFilesystem` *does* expose an
append/tail event plane (`root.rs:29-30`, `append`/`tail` with `SeqNo`), and
`crates/events/` holds a full event-log/projection stack, but the transcript
does not ride on it: transcript writes go through `put` with a CAS expectation.

## Keying and identity

A thread is addressed by **scope plus thread id**, and the scope is structural,
not a string blob (`contract.rs`, `ThreadScope`):

```rust
pub struct ThreadScope {
    pub tenant_id: TenantId,
    pub agent_id: AgentId,
    pub project_id: Option<ProjectId>,
    pub owner_user_id: Option<UserId>,
    pub mission_id: Option<MissionId>,
}
```

Those axes are projected directly into the path prefix
(`filesystem_service.rs:2997-3003`, `thread_root_string` = `scope_axes_string`
+ `/threads/<thread_id>`), which is why the on-disk layout above shows optional
`agents/`, `projects/`, `owners/`, `missions/` segments. Note what is *not* in
the key: no working directory, no cwd hash, no git worktree. IronClaw keys
sessions by tenant, agent, project, and owner. There is nothing to reconcile on
a moved directory because the filesystem path was never part of the identity.
`AgentId` being a first-class scope axis (`storage-placement.md`) is the
distinctive part: a thread belongs to an *agent*, not merely to a user.

`ThreadId` is a validated string id, not a mandated UUID
(`crates/contracts/ironclaw_host_api/src/ids.rs:213`):

```rust
string_id!(ThreadId, "thread", validate_scope_id);
```

`validate_scope_id` bounds it at 256 bytes and rejects path separators, control
characters, and the reserved `__ironclaw_` sentinel prefix, because ids become
path segments. Callers may therefore supply their own id; the service mints one
only when absent, as UUIDv4 (`filesystem_service.rs:3137`):

```rust
fn generated_thread_id() -> ThreadId { ThreadId::new(uuid::Uuid::new_v4().to_string()) }
```

So the id scheme encodes neither ordering nor location. Ordering comes from the
per-thread `sequence`, and location from the scope prefix.

**Listing is scoped, and scoping is a security control rather than a
convenience.** `list_threads_for_scope` carries a normative requirement
(`service.rs:352-372`):

> Implementations MUST scope the listing by `owner_user_id` (or equivalent
> caller-binding fields on the scope) — otherwise a caller could enumerate
> threads owned by other users in the same `(tenant, agent, project)` triple.

There is no cross-tenant or cross-agent enumeration path. Reads are further
non-enumerating: `read_thread` must return the *same* `UnknownThread` error for
"does not exist" and "exists but is owned by another scope"
(`service.rs:285-289`) "so callers cannot use the response as an existence
oracle". `resolve_scope`/`read_thread_by_id` exist for the trusted-internal
case where a bare `ThreadId` must be mapped back to its scope, and both are
opt-in per backend, with `supports_resolve_scope()` defaulting to `false`
(`service.rs:317-331`).

## The store interface

The interface is genuinely pluggable: `SessionThreadService` is a public
`#[async_trait]` trait, and the on-disk/SQL implementation is one implementor
among several (there are in-memory and stub implementors for tests, and a
blanket `impl SessionThreadService for Arc<S>` at `service.rs:375-380`). Below
is the trait verbatim, with default method bodies elided and marked; a method
with a default body is optional for a backend, and the defaults either fail
closed with `SessionThreadError::Backend` or compose other required methods.

```rust
/// Canonical Reborn session thread and transcript boundary.
#[async_trait]
pub trait SessionThreadService: Send + Sync {
    async fn ensure_thread(&self, request: EnsureThreadRequest)
        -> Result<SessionThreadRecord, SessionThreadError>;                          // required

    async fn accept_inbound_message(&self, request: AcceptInboundMessageRequest)
        -> Result<AcceptedInboundMessage, SessionThreadError>;                       // required

    async fn replay_accepted_inbound_message(&self, request: ReplayAcceptedInboundMessageRequest)
        -> Result<Option<AcceptedInboundMessageReplay>, SessionThreadError>;         // required

    async fn mark_message_submitted(&self, scope: &ThreadScope, thread_id: &ThreadId,
        message_id: ThreadMessageId, turn_id: String, turn_run_id: String)
        -> Result<ThreadMessageRecord, SessionThreadError>;                          // required

    async fn mark_message_rejected_busy(&self, scope: &ThreadScope, thread_id: &ThreadId,
        message_id: ThreadMessageId)
        -> Result<ThreadMessageRecord, SessionThreadError>;                          // required

    async fn mark_message_queued(&self, scope: &ThreadScope, thread_id: &ThreadId,
        message_id: ThreadMessageId, active_run_id: String)
        -> Result<ThreadMessageRecord, SessionThreadError>;                          // default: fails closed

    async fn read_thread_message(&self, scope: &ThreadScope, thread_id: &ThreadId,
        message_id: ThreadMessageId)
        -> Result<Option<ThreadMessageRecord>, SessionThreadError>;                  // default: fails closed

    async fn append_assistant_draft(&self, request: AppendAssistantDraftRequest)
        -> Result<ThreadMessageRecord, SessionThreadError>;                          // required

    async fn append_finalized_assistant_message(&self, request: AppendFinalizedAssistantMessageRequest)
        -> Result<ThreadMessageRecord, SessionThreadError>;                          // default: draft + finalize

    async fn append_tool_result_reference(&self, request: AppendToolResultReferenceRequest)
        -> Result<ThreadMessageRecord, SessionThreadError>;                          // required

    async fn append_capability_display_preview(&self, request: AppendCapabilityDisplayPreviewRequest)
        -> Result<ThreadMessageRecord, SessionThreadError>;                          // required

    async fn update_tool_result_reference(&self, request: UpdateToolResultReferenceRequest)
        -> Result<ThreadMessageRecord, SessionThreadError>;                          // required

    async fn put_tool_result_record(&self, request: PutToolResultRecordRequest)
        -> Result<(), SessionThreadError>;                                           // default: fails closed
    async fn read_tool_result_record(&self, request: ReadToolResultRecordRequest)
        -> Result<Option<ToolResultRecordChunk>, SessionThreadError>;                // default: fails closed
    async fn update_tool_result_record(&self, request: UpdateToolResultRecordRequest)
        -> Result<(), SessionThreadError>;                                           // default: fails closed
    async fn delete_tool_result_record(&self, request: DeleteToolResultRecordRequest)
        -> Result<(), SessionThreadError>;                                           // default: fails closed

    async fn update_assistant_draft(&self, request: UpdateAssistantDraftRequest)
        -> Result<ThreadMessageRecord, SessionThreadError>;                          // required

    async fn finalize_assistant_message(&self, scope: &ThreadScope, thread_id: &ThreadId,
        message_id: ThreadMessageId, content: MessageContent)
        -> Result<ThreadMessageRecord, SessionThreadError>;                          // required

    async fn redact_message(&self, request: RedactMessageRequest)
        -> Result<ThreadMessageRecord, SessionThreadError>;                          // required

    async fn load_context_window(&self, request: LoadContextWindowRequest)
        -> Result<ContextWindow, SessionThreadError>;                                // required
    async fn load_context_messages(&self, request: LoadContextMessagesRequest)
        -> Result<ContextMessages, SessionThreadError>;                              // required

    async fn list_thread_history(&self, request: ThreadHistoryRequest)
        -> Result<ThreadHistory, SessionThreadError>;                                // required

    async fn list_thread_messages_bounded(&self, request: BoundedThreadMessagesRequest)
        -> Result<BoundedThreadMessages, SessionThreadError>;                        // default: fails closed
    async fn list_thread_messages_range(&self, request: ThreadMessageRangeRequest)
        -> Result<ThreadMessageRange, SessionThreadError>;                           // default: filter full history
    async fn latest_thread_message(&self, request: LatestThreadMessageRequest)
        -> Result<Option<ThreadMessageRecord>, SessionThreadError>;                  // default: scan full history
    async fn finalized_assistant_message_by_run(&self, request: FinalizedAssistantMessageByRunRequest)
        -> Result<Option<ThreadMessageRecord>, SessionThreadError>;                  // default: scan full history

    async fn read_thread(&self, request: ThreadHistoryRequest)
        -> Result<SessionThreadRecord, SessionThreadError>;                          // default: full history, take thread
    async fn delete_thread(&self, scope: &ThreadScope, thread_id: &ThreadId)
        -> Result<(), SessionThreadError>;                                           // default: fails closed

    async fn create_summary_artifact(&self, request: CreateSummaryArtifactRequest)
        -> Result<SummaryArtifact, SessionThreadError>;                              // required

    fn supports_resolve_scope(&self) -> bool;                                        // default: false
    async fn resolve_scope(&self, thread_id: ThreadId)
        -> Result<ThreadScope, SessionThreadError>;                                  // default: fails closed

    async fn update_thread_goal(&self, request: UpdateThreadGoalRequest)
        -> Result<ThreadGoal, SessionThreadError>;                                   // default: fails closed
    async fn read_thread_by_id(&self, thread_id: ThreadId)
        -> Result<SessionThreadRecord, SessionThreadError>;                          // default: fails closed
    async fn list_threads_for_scope(&self, request: ListThreadsForScopeRequest)
        -> Result<ListThreadsForScopeResponse, SessionThreadError>;                  // default: fails closed
}
```

Source: `crates/domains/ironclaw_threads/src/service.rs:22-373`. Most of the
trait ships fail-closed defaults, leaving a small required core. Three
properties are worth extracting:

- **Fail-closed defaults over silent lies.** The default
  `list_threads_for_scope` returns `Backend(...)` deliberately, "so backends
  that do not yet implement enumeration surface a clear `503 Service
  Unavailable` at the gateway instead of pretending the caller has zero
  threads" (`service.rs:352-357`).
- **Performance contracts written into the trait.** `read_thread` exists purely
  because using `list_thread_history` as an ownership probe "on a large thread
  is hundreds of rows per second per active stream" (`service.rs:271-284`).
  `list_thread_messages_bounded` requires the budget to be enforced *while*
  reading: "Implementations must enforce the budget while reading, rather than
  materializing an unbounded transcript and checking afterward"
  (`service.rs:200-203`).
- **No fork, no rewind, no branch, no message edit.** Not deferred behind a
  flag: the operations do not exist. `grep -i "rewind\|fork"` over
  `crates/domains/ironclaw_threads/src` and over `docs/reborn/` returns nothing
  for the transcript boundary. The only retroactive operation is
  `redact_message`.

Below the domain trait sits the substrate contract that supplies the durability
primitives. `RootFilesystem` (`crates/substrates/ironclaw_filesystem/src/root.rs:37`)
is one trait implemented by every backend (local disk, Postgres, libSQL, HSM,
in-memory) *and* by the routing dispatcher, since "the dispatcher *is* a backend
that routes by longest-prefix mount" (`root.rs:11-18`). Its planes
(`root.rs:19-35`): a unified entry plane (`put`, `get`, `delete`,
`delete_if_version`, `list_dir`, `list_dir_bounded`, `query`, `query_ordered`,
`ensure_index`, `stat`, `read_file_bounded`), an atomicity plane (`begin` →
`StorageTxn`), an event plane (`append`, `tail`), and a legacy bytes plane being
removed. The floor is CAS, not transactions (`root.rs:26-28`):

> **Atomicity** — [`begin`] for backends that natively support multi-key
> transactions. Stores must always work with CAS (`put` +
> `CasExpectation::Version`) as the floor.

`CasExpectation` is the precondition type the transcript writes on
(`record.rs:236-256`), with a deliberate design note:

> All multi-step store operations (lease claim, lease consume, status
> transitions) are implemented with `CasExpectation::Version` and retry on
> [`FilesystemError::VersionMismatch`]. Closure-based transactions across async
> boundaries are intentionally absent [...] consumers must continue to work when
> only CAS is available.
>
> ```rust
> pub enum CasExpectation {
>     Absent,                   // Path must not currently hold an entry. Used for issue/create.
>     Version(RecordVersion),   // Path must currently hold the named version.
>     Any,                      // Overwrite regardless. Used only by backfills / admin flows.
> }
> ```

`RecordVersion` is backend-minted and unforgeable by consumers
(`record.rs:180-192`): "Consumers obtain versions only by reading existing
entries — they cannot fabricate one." Reads return it attached
(`VersionedEntry { path, entry, version }`, `record.rs:345-350`) so a
read-modify-write always has a precondition available. The store also documents
the ABA hazard on version-conditioned deletes rather than papering over it
(`root.rs:184-191`): "Version tokens are not generation-stable: a path's version
restarts at 1 on a fresh put after a prior delete."

## Write and append path (ordering, durability, concurrency, delivery)

**Commit shape.** New messages are created with `put(..., CasExpectation::Absent)`
(`filesystem_service.rs:491`, `2120`, `633`), i.e. insert-if-absent. Mutations
in place (draft → finalize, status transitions, redaction) go through
`apply_message_update`, which reads the versioned record and writes back with
`CasExpectation::Version(versioned.version)` (`filesystem_service.rs:1223-1250`).
Nothing rewrites the whole thread. There is no append-to-a-log step.

**Ordering** is a durable per-thread `u64` sequence, and IronClaw has two
mechanisms for allocating it. On backends that expose the native
`ReserveSeq` operation, a path-local counter row at
`<thread_root>/message_sequence` is bumped atomically; otherwise it falls back
to a CAS loop on the thread record's `next_sequence`
(`filesystem_service.rs:1083-1130`). The thread record itself carries the
counter (`filesystem_service.rs:125-130`):

```rust
struct StoredThreadRecord {
    #[serde(flatten)]
    record: SessionThreadRecord,
    next_sequence: u64,
}
```

The migration reasoning is instructive about why a "just switch to the faster
counter" change is unsafe once sequences exist
(`filesystem_service.rs:1098-1109`):

> Migration safety: a thread that already assigned message sequences under the
> legacy per-thread-record counter (`next_sequence > 1`) must keep using it. The
> native path-local counter starts at 1 for a path with no row, so switching an
> *existing* thread onto it would restart at 1 and collide with messages already
> at sequences 1..N — corrupting ordering and clobbering the sequence index [...]
> No thread ever switches counters mid-stream.

The legacy fallback is explicitly named as a bottleneck: "This preserves
compatibility but retains the old shared-thread-record CAS bottleneck"
(`filesystem_service.rs:1132-1134`), with `FILESYSTEM_CAS_RETRIES: usize = 8`
(`filesystem_service.rs:94`) before surfacing "filesystem CAS retries
exhausted".

**Atomicity.** Where the backend supports transactions, the accept path commits
the counter, the message record, the sequence index, and the idempotency record
together (`filesystem_service.rs:519-560`, `1455-1458`):

> On transactional backends the thread counter, message, sequence index, and
> idempotency record commit together; fallback backends reserve immediately
> before the legacy message write.

The non-transactional path degrades to a documented ordering, with the message
as the authority (`filesystem_service.rs:1530-1532`): "the message is
authoritative, and the idempotency record accelerates later replays when it can
be written."

**Concurrency.** Three layers, worth separating:

1. At the record level, optimistic concurrency with a real expected-version
   precondition (`CasExpectation::Version`) and bounded retry. This is a genuine
   expected-version write boundary, not advisory.
2. At the *sequence* level, an atomic counter reservation, so two concurrent
   writers get distinct sequences rather than one losing.
3. At the *turn* level, a single-active-run lock keyed by the canonical
   `TurnScope` of tenant, agent, optional project, thread
   (`turn-persistence.md:47-55`). `crates/Architecture.md` lists it among the
   key invariants: "One active run per canonical thread is enforced before
   model/tool side effects." Concurrent user messages during an active run are
   not merged into a race; they are either steered into a queue
   (`MessageStatus::Queued` via `mark_message_queued`) or refused
   (`RejectedBusy`), which is a store-visible status, not just an HTTP code.

**Delivery semantics.** At-least-once inbound with an idempotency record that
makes acceptance effectively exactly-once per external event. The key is a
SHA-256 of the full tuple (`filesystem_service.rs:1437-1440`):

> First, check idempotency. The on-disk key SHA-256s the full (scope,
> source_binding_id, external_event_id) tuple, so a same-binding/event from a
> different scope hashes to a different key (and we only see records under the
> current MountView).

If the tuple is incomplete (`source_binding_id` or `external_event_id` absent),
no idempotency key is formed at all and the write is unguarded
(`filesystem_service.rs:1421-1428`), a deliberate choice that shifts
responsibility to callers that omit event ids. On a transactional backend, a
duplicate surfaces as `TransactionalMessageWrite::IdempotencyAlreadyAccepted`
and the prior `AcceptedInboundMessage` is returned instead
(`filesystem_service.rs:1506-1523`). The returned value carries
`idempotent_replay: bool` so the caller can tell a fresh accept from a replay.

**Advisory writes are marked as such.** After a durable accept, the recency
stamp and derived sidebar title are best-effort, with the reason stated
(`filesystem_service.rs:1572-1574`): "silent-ok: the message is already durable;
the recency stamp and derived label are advisory, and failing the accept here
could make an un-idempotent caller retry and duplicate the message." The
counter-example is redaction, where the same class of derived copy is *not*
best-effort (`filesystem_service.rs:2425-2429`): "Propagating the removal is a
redaction obligation, not best-effort: failing here is correct if the copy
cannot be cleared."

## Read and resume path

There is no single "resume" call that rehydrates a session. There are four
distinct read shapes, chosen by what the caller needs:

- **`load_context_window`** is the model-facing read. Reads the latest N messages
  via the ordered index, loads summary artifacts, and applies summary
  replacement over the range before truncating to `max_messages`
  (`filesystem_service.rs:2435-2467`). This is the only path that interprets
  summaries. It checks the one-shot cache first (seeded when the accepted message
  is `sequence == 1`, so a brand-new thread's first turn avoids a read round
  trip: `filesystem_service.rs:1546-1550`).
- **`load_context_messages`** is an explicit by-id fetch of a caller-chosen set,
  read concurrently with `join_all` and returned in the requested order
  (`filesystem_service.rs:2469-2493`).
- **`list_thread_history`** is the full transcript plus summary artifacts plus
  the thread record with its index overlay (`filesystem_service.rs:2495-2518`).
  Unbounded, and the trait steers callers away from it for hot paths.
- **`list_thread_messages_range` / `list_thread_messages_bounded` /
  `latest_thread_message`** are cursor-ish reads served from the ordered index
  (`list_thread_messages_range_indexed`, `filesystem_service.rs:731`), with a
  byte budget for export.

Resume reads the durable store; there is no local-cache-first path, and the
only cache is the in-process one-shot window described above. Every read begins
by resolving the thread record under the caller's exact scope
(`read_thread_versioned`), so an unauthorized read fails before any transcript
materializes.

The byte-budget denominator for bounded reads is the *stored* representation,
not the domain type (`stored_message.rs`): "Both durable and in-memory bounded
reads use this stored representation as their byte-budget denominator." That
matters because `StoredThreadMessageRecord` re-adds a field the wire type skips
(`tool_result_provider_call` is `#[serde(skip_serializing)]` on
`ThreadMessageRecord`), so the two would otherwise disagree about size.

Loop-execution resume is a separate mechanism on the other boundary: the driver
returns `LoopExit::Blocked`, the host persists an opaque checkpoint ref plus a
bounded payload, and `resume_turn` requeues the *same* run against that
checkpoint (`Architecture.md:589-604`). Checkpoints are scoped and
non-transferable (`turn-persistence.md:96-98`): "Reads with a matching ref but
foreign scope or run return no state."

## Listing, summaries, and search

Listing is served by a **maintained index projection**, not a directory scan.
`ThreadIndexRecord` flattens the thread record and adds derived fields
(`filesystem_service/thread_index.rs:1-90`):

```rust
struct ThreadIndexRecord {
    #[serde(flatten)]
    record: SessionThreadRecord,
    next_sequence: u64,
    flags: ...,
    derived_title: Option<String>,
}
```

with indexed keys `scope_key`, `activity_sort`, and `thread_id`, and
`THREAD_INDEX_KNOWN_ROW_MAX = 100_000`. The index row is declared the recency
authority for listing (`filesystem_service.rs:1176-1181`): "The index row is
the recency authority for listing; avoiding a full `thread.json` CAS here keeps
activity writes row-shaped."

The denormalized field that carries the most weight is `derived_title`, written
at message-accept time specifically to avoid an N+1 on the sidebar
(`filesystem_service.rs:1552-1562`). Only the first user message may seed it,
and the code explains the alternative it rejected: seeding from whatever arrives
next "would show the newest message where the sidebar contract promises the
first". Threads that predate the field are healed lazily on listing, with a
bounded read fan-out (`TITLE_DERIVATION_READ_CONCURRENCY: usize = 8`,
`filesystem_service.rs:107`) deriving titles for at most the page being returned
(`filesystem_service.rs:2782-2810`). Pagination is capped:
`LIST_THREADS_DEFAULT_PAGE_SIZE = 50`, `LIST_THREADS_MAX_PAGE_SIZE = 200`
(`filesystem_service.rs:2859-2860`), clamped rather than rejected.

Consistency between index and record is maintained by writing the index row on
the same events that change the record, plus the idempotent rebuild pass in
`transcript_migration.rs` (`TRANSCRIPT_PAGE_CONFLICT_RETRIES = 5`, conflicts
being `VersionMismatch | BackendBusy`, with completion markers so the rebuild
runs once). Index divergence is therefore recoverable by design: the rows are
derivable from the records.

**Search: absent.** There is no FTS index, no vector index, and no
`search_threads` operation on the trait. Listing is by scope ordered by
activity, and that is the whole retrieval surface for transcripts. Memory
documents are a separate substrate (`docs/reborn/contracts/memory.md`), not a
transcript search index.

## Entry/message structure and versioning

The stored message is a typed record, not an opaque blob
(`contract.rs`, `ThreadMessageRecord`):

```rust
pub struct ThreadMessageRecord {
    pub message_id: ThreadMessageId,
    pub thread_id: ThreadId,
    pub sequence: u64,
    pub kind: MessageKind,
    pub status: MessageStatus,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub actor_id: Option<ActorId>,
    pub source_binding_id: Option<SourceBindingId>,
    pub reply_target_binding_id: Option<ReplyTargetBindingId>,
    pub turn_id: Option<String>,
    pub turn_run_id: Option<String>,
    pub tool_result_ref: Option<String>,
    #[serde(skip_serializing)]
    pub tool_result_provider_call: Option<serde_json::Value>,
    pub content: Option<MessageContent>,
    pub attachments: Vec<AttachmentRef>,
    pub redaction_ref: Option<String>,
}
```

with two enums doing the discrimination:

```rust
pub enum MessageKind { User, Assistant, System, Summary, CheckpointReference,
                       ToolResultReference, CapabilityDisplayPreview }

pub enum MessageStatus { Accepted, Queued, Submitted, RejectedBusy, DeferredBusy,
                         Draft, Finalized, Interrupted, Superseded, Redacted, Deleted }
```

`DeferredBusy` is explicitly legacy, superseded by `Queued` steering
(`conversation-binding.md`), and there is a test-only injector for it
(`inject_legacy_deferred_busy_for_test`, `filesystem_service.rs:1299`) so the
read path's handling of old rows stays covered.

The store **parses and interprets** entries. It relies on `sequence` for
ordering, on `kind`/`status` for every projection and query
(`latest_thread_message` filters on both), on `turn_run_id` for
`finalized_assistant_message_by_run`, and on `message_id` for identity. Dedup,
by contrast, keys off the separate idempotency record, not off a field of the
message.

Payload boundaries are enforced in the contract layer: `MessageContent` carries
text plus `Vec<AttachmentRef>` where "Attachments are carried as references only
— never raw bytes", extracted text is capped at
`MAX_EXTRACTED_TEXT_CHARS = 200_000`, and `GoalStatement` at 4000 chars. Large
tool outputs go out-of-band: the message holds a `tool_result_ref`, and the
payload lives at `tool_results/sha256-<hex>.bin` read back in chunks
(`ToolResultRecordChunk`), addressed by SHA-256 of the ref
(`filesystem_service.rs:2928-2949`).

**Format evolution** is handled three ways, all visible in the code rather than
in a version field:

1. **Additive serde with optional legacy fields.** `created_at`/`updated_at` are
   `Option<DateTime<Utc>>` for rows that predate durable timestamps, guarded by
   validators named for the two failure modes (`MissingDurableTimestamps`,
   `ClearedDurableTimestamps`) so new writes cannot regress to `None`.
2. **Behavioral migration gates rather than rewrites.** The `next_sequence > 1`
   check that pins a thread to its original counter for life is the clearest
   example: old and new coexist per-thread, forever, with no rewrite.
3. **SQL migrations for the substrate**, `V1..V34`, additive with
   `ADD COLUMN IF NOT EXISTS` and safe defaults
   (`V28__root_filesystem_records.sql`), plus projection-adding migrations
   (`V30__root_filesystem_events.sql`,
   `V33__root_filesystem_ordered_index_rows.sql`).

There is **no session-format version number** on the thread or message record,
and no format-version negotiation. The ratchet is one-way in practice (a
migrated thread keeps its counter; rebuilt projections assume the current
shape), but nothing declares it as such. Flagged under open questions.

## Compaction and history management

Compaction produces a **new record and never touches the compacted messages**.
`SummaryArtifact` (`contract.rs`) is stored at
`summaries/<summary_id>.json` and spans a closed sequence range:

```rust
pub struct SummaryArtifact {
    pub summary_id: SummaryArtifactId,
    pub thread_id: ThreadId,
    pub start_sequence: u64,
    pub end_sequence: u64,
    pub summary_kind: SummaryKind,               // Compaction
    pub content: String,
    pub model_context_policy: SummaryModelContextPolicy,  // ReplaceRangeWhenSelected
}
```

`create_summary_artifact` validates that the range is non-empty and that both
endpoints actually exist as messages, then writes with
`CasExpectation::Absent` (`filesystem_service.rs:2682-2752`). Overlap handling
is idempotent-by-content: replaying the identical compaction returns the
existing artifact, while a genuinely different overlapping range is rejected
with `OverlappingSummaryRange`, and only `ReplaceRangeWhenSelected` summaries
are overlap-checked at all (`summary_artifacts.rs:1-70`).

The **model-visible view** shrinks only inside `load_context_window`, where
`context_messages_with_summary_replacements` substitutes the summary for its
range (`filesystem_service.rs:2458`). Nothing else in the system sees a
shortened thread: `list_thread_history` returns both the messages and the
summary artifacts, so a UI or an export crosses a compaction boundary by seeing
*both* the original messages and the summary that stands in for them. Resume
across a compaction boundary is therefore lossless in the durable record and
lossy only in the assembled prompt.

Compaction placement is unusual and worth extracting. It is a **host** concern,
not the loop's, and it is treated as a security boundary as much as a context
one (`docs/reborn/contracts/agent-loop-protocol.md:200-270`): "Host-managed
compaction is a typed retention boundary." Compaction scans for leaked secrets
with actions `Block` / `Redact` / `Warn`, replaces matches in place, rescans
before inference and before persistence, and fails closed if matches remain;
it reports an additive `redacted_leak_count` and one safe
`CompactionLeakDetected` milestone. No other product in this corpus runs a leak
scanner inside the compaction path.

## Rewind, checkpoints, and fork

**Rewind: absent. Fork: absent. Branch: absent. Message edit: absent.** These
are not deferred features with reserved enum variants in the transcript layer;
the operations do not appear in the trait, the implementation, or the design
docs. The one place `Fork` appears anywhere adjacent is the *subagent* context
seed mode, and it is reserved and unimplemented
(`docs/reborn/subagent-spawn/README.md:74`): "`Fork` seed mode (full
parent-context copy) — enum variant reserved, unimplemented", with
`phase-3-integration.md:677` making the runtime consequence explicit: "`Fork` is
reserved/unimplemented -> Denied."

The only retroactive operation on the transcript is `redact_message`, and it is
a **destructive in-place edit**, not an appended tombstone
(`filesystem_service.rs:2399-2432`): status becomes `Redacted`, and `content`,
`attachments`, and `tool_result_provider_call` are all set to `None`/empty,
leaving a `redaction_ref` pointer. Summary content gets the same treatment via
`REDACTED_SUMMARY_CONTENT = "[redacted]"` (`filesystem_service.rs:3259`). A
reader after redaction cannot reconstruct what was there, which is the point,
and which is the exact opposite of what an append-only log would give you.

**Checkpoints exist, but they check point the loop, not the transcript.** They
live on the process journal, hold the loop driver's serialized
`LoopExecutionState` as bounded opaque payload bytes, and are written when a
capability requires approval or auth (`turn-persistence.md:86-101`,
`Architecture.md:589-604`). Two properties matter for comparison with
file-state checkpointing elsewhere in this corpus:

- They are **not** environment or file-state snapshots. There is no
  content/diff/hash store of workspace files tied to a turn.
- The payload is opaque to the store, bounded, and debug-redacted, and only the
  ref and schema metadata are ever projected
  (`turn-persistence.md:99-101`).

Lease expiry, the closest thing to an involuntary rewind, is deliberately
terminal rather than resumable (`Architecture.md:620-632`):

> ```text
> runner crashes or stops heartbeating
>   -> reconciler sees expired Running/CancelRequested lease
>   -> Running          => terminal Failed (sanitized "lease_expired")
>   -> CancelRequested  => terminal Cancelled
>   -> the terminal transition releases the active-thread lock
> ```
>
> Reborn does not automatically retry uncertain side-effecting work after a lost
> lease — expiry is terminal, and the user resubmits explicitly.

Note a live doc conflict here: `turn-persistence.md:91` still says expired
leases "transition to `RecoveryRequired` [...] and keep the active lock", while
`Architecture.md:578-579` states `RecoveryRequired` "survives only as a legacy
variant" and expiry is terminal. Per the repo's own precedence rule, the
architecture doc and the code win.

## Subagents and nested sessions

A subagent gets a **first-class sibling thread**, not a nested transcript and
not entries in the parent's transcript. The child's `TurnScope` copies
tenant, agent, and project verbatim, and only `thread_id` is fresh
(`docs/reborn/subagent-spawn/README.md`), so the child's records land under the
same scope prefix as any other thread of that agent. The child also inherits
`owner_user_id`, which is what makes approvals surface on the child thread
rather than leaking to another user.

The durable parent-child link lives on the **run**, not the thread:
`parent_run_id`, `subagent_depth`, and `spawn_tree_root_run_id` are fields of
the child run in the process journal. So the transcript store itself does not
know it is holding a child session; the lineage is a property of the
coordination plane. The gate is a capability call
(`spawn_subagent(flavor_id, task, handoff?)`) returning
`CapabilityOutcome::AwaitDependentRun`, which awaits the entire child set and
resolves inline if all children are already terminal.

Transcript isolation is total by default: the child starts with an **empty grant
and lease set**, and the seed is either `Fresh` (goal only) or
`Handoff(String)` (a curated blob re-materialized into the child scope). The
capability allowlist of the child's profile is described as "a surface *ceiling*,
not authority". The goal is placed as the child's first **user** message, and the
reason is stated: "Never the system message — the goal is model-generated and may
carry upstream-tainted content."

Nesting is bounded four ways, all enforced before `submit_turn`:
`allow_nesting = false` by default, a depth cap, a per-turn fan-out cap, and an
atomic `reserve_tree_descendants(scope, root, delta, cap)` against
`MAX_TREE_DESCENDANTS` backed by a durable `SpawnTreeReservation`
(`phase-2-mechanisms.md:1884` names the threat model directly: "Fork-bomb via
depth × fan-out").

Cascade behavior on parent cancellation is the part most relevant to us: the
child's result is not silently dropped but recorded, via
`SubagentResultTombstone { child_run_id, disposition: "discarded_by_parent_cancel",
terminal_status }`. That is a durable record of a discarded child, which is
strictly more than "orphan" or "cascade delete".

Two caveats. First, spawning is **off in every shipped profile** at this commit:
`builtin.spawn_subagent` is deny-filtered via `TEMP(disable-spawn-subagents)` in
`crates/loop/ironclaw_turn_runner/src/runtime.rs`, per the 2026-07 status note in
`crates/Architecture.md`. Second, parent *delete* cascade is unspecified:
`delete_thread` deletes one thread's subtree and index row and says nothing about
children spawned from it (see open questions).

## Retention, deletion, and multi-host

**Retention.** There is no TTL, lifecycle policy, or scheduled cleanup for
transcripts. Threads and messages persist until explicitly deleted. Retention
exists only on the *other* boundary, and only for released admission
reservations (`turn-persistence.md:80`): "Released reservation evidence is
retained only while the corresponding terminal run remains within the bounded
terminal-record retention window; active capacity accounting must not scan
unbounded released history." Bounding, elsewhere, is applied to reads (page
sizes, byte budgets, `THREAD_INDEX_KNOWN_ROW_MAX`) rather than to stored
history.

**Deletion** is scope-checked, subtree-wide, and index-cascading
(`filesystem_service.rs:2643-2680`): probe ownership through `read_thread`
(preserving the non-enumerating error shape), delete the thread root
recursively, invalidate the context cache, then delete the index record. A
missing thread still triggers an index-record delete before returning
`UnknownThread`, which self-heals a stale row. Messages, summaries, tool-result
payloads, and the sequence counter all live under the thread root and go with
it.

One asymmetry follows from the layout and is worth stating plainly as our
reading of the code, not as a documented behavior: inbound idempotency records
live at `/threads/idempotency/<sha256>.json`
(`filesystem_service.rs:2993-2995`), which is *outside* any thread root
(`thread_root_string` = scope axes + `/threads/<thread_id>`,
`filesystem_service.rs:2997-3003`). A `delete_thread` therefore leaves the
thread's idempotency records behind, pointing at a `thread_id` and `message_id`
that no longer exist. The replay path reads such a record and reconstructs an
`AcceptedInboundMessage` from it (`accepted_message_from_idempotency_record`,
`filesystem_service.rs:1043`), so a redelivery of a pre-deletion external event
can still be reported as an idempotent replay of a deleted message. We found no
sweeper for these records and no documented TTL on them.

**Multi-host is a first-class path, not a workaround**, and this is the sharpest
contrast with the file-per-session products in this corpus. Nothing in the
design assumes a shared local filesystem: `RootFilesystem` backends include
libSQL and Postgres (`profiles/local.toml` sets
`database_backend = "libsql"`; `server.toml` and `server-multitenant.toml` are
the multi-tenant deployments), and correctness rests on primitives that work
across processes:

- CAS with backend-minted versions, so two hosts writing the same record cannot
  lose an update (`record.rs:236-256`).
- Atomic path-local sequence reservation, so two hosts appending to one thread
  get distinct sequences.
- Runner leases with lease tokens and heartbeats
  (`DEFAULT_TURN_RUNNER_HEARTBEAT_INTERVAL = 5s`,
  `DEFAULT_TURN_RUNNER_POLL_INTERVAL = 200ms`,
  `crates/app/ironclaw_composition/src/runtime_input.rs`), where "Heartbeats
  only renew metadata for matching, unexpired runner ID/lease token"
  (`turn-persistence.md:89`), and liveness decisions must use durable lease
  metadata rather than one event per heartbeat (`turn-persistence.md:90`).
- A durable active-thread lock, so crash detection is a lease-expiry
  reconciliation rather than a stale-PID heuristic.

The local-disk backend is treated as the constrained case rather than the
reference case: it must fail closed on the CAS gate, and where it cannot serve
an operation (`ReserveSeq`) the domain layer degrades to the CAS fallback.

## Interop with foreign session stores

No. IronClaw does not discover, import, or resume any other product's session
store, and there is no converter. The only foreign-agent artifact in the tree is
a legacy pre-Reborn sandbox path that persisted streamed events from an external
coding agent for job-detail replay
(`migrations/V5__claude_code.sql`: a `claude_code_events` table keyed by
`job_id`, alongside `agent_jobs.job_mode`). That is job telemetry for a sandbox
runner, not a session transcript store, and it is not part of the Reborn
transcript boundary.

## What this implies for our Session Store (our inference)

Our reading: in IronClaw, a stored session is **a scope-owned set of
individually versioned records under a virtual path prefix**: one metadata
record holding a durable sequence counter, one record per message, one record
per summary artifact, plus rebuildable index projections, where every write
carries a compare-and-swap precondition and every read is filtered through the
caller's exact scope. It is deliberately *not* an append-only log: history is
mutable in place under CAS (draft → finalize, status transitions, redaction),
and the durable record is expected to shrink under a redaction obligation.

Five things this changes or sharpens for our design.

1. **A real expected-version write boundary is achievable without event
   sourcing, and IronClaw is the corpus's proof.** `CasExpectation::Version`
   with backend-minted, unforgeable `RecordVersion` tokens and a bounded retry
   loop gives lost-update safety at the store's write boundary. Our synthesis
   previously observed that no product implemented optimistic concurrency at
   that boundary; IronClaw does, and it does so while keeping the store
   mutable. Worth noting that CAS-per-record is *weaker* than a per-aggregate
   expected-sequence append: it protects each record, not the invariant across
   records, which is exactly why IronClaw needs a *separate* active-run lock at
   the turn level. If our Session Store is a decider aggregate with an
   expected-version append, we get both properties from one mechanism, and that
   is a concrete argument for our chosen shape rather than against it.
2. **Separate the lifecycle log from the transcript log, and enforce the
   separation as a redaction rule.** IronClaw's strongest structural idea is
   that "what ran" (leases, checkpoints, admission, idempotency) and "what was
   said" (messages) are different stores with a stated one-way information flow:
   turn records hold refs and metadata only, never prompt or assistant content.
   That gives them a lifecycle log they can retain, project, and replay freely
   without it becoming a shadow copy of the transcript, and it makes transcript
   redaction tractable because there is only one place content lives. Our
   event-sourced Session Store should adopt the same rule explicitly: lifecycle
   events carry references, content lives in exactly one place.
3. **Redaction is the requirement that punishes append-only designs, and
   IronClaw shows the cost of answering it destructively.** They satisfy
   redaction by destroying content in place and propagating removal to every
   derived copy as a hard obligation (the derived title is cleared
   non-best-effort; summary content is replaced with `[redacted]`). We cannot
   copy that directly on an append-only log, so we need the equivalent
   guarantee by other means: content addressed indirectly so a single
   crypto-erase or payload delete satisfies redaction across all events that
   reference it, plus the same non-best-effort propagation rule for
   projections. Any projection holding a copy of content is a redaction
   liability, and IronClaw's `derived_title` handling is the pattern to follow.
4. **Idempotency records must be owned by the aggregate they protect.**
   IronClaw's inbound dedup by SHA-256 of `(scope, source_binding_id,
   external_event_id)` is the right key shape, and the fact that those records
   live outside the thread root, survive thread deletion, and have no sweeper is
   a defect we should avoid by construction: dedup state belongs inside the
   aggregate's key space so deletion and retention cascade to it. It also argues
   for our dedup key to be part of the appended event rather than a sidecar,
   which an event-sourced design makes natural.
5. **Cascade semantics for children are answerable, and the answer is a durable
   tombstone.** IronClaw links children at the *run* level
   (`parent_run_id`, `spawn_tree_root_run_id`), bounds trees with an atomic
   descendant reservation, and records a `SubagentResultTombstone` when a parent
   cancel discards a completed child's result. That is the closest thing in the
   corpus to closing the subagent-cascade gap, and it is a good model for us:
   record the discard, do not orphan it. What IronClaw does *not* answer is
   transcript-level cascade on parent delete, which leaves that half of the gap
   open for us too.

One negative result is also useful: IronClaw ships no rewind, no fork, no
branch, and no message edit, and pays no visible architectural cost for their
absence. Combined with the rest of the corpus, that is evidence that fork and
rewind are product features to be scheduled on demand, not properties the
durable model must be shaped around from day one.

## Open questions

- **Format versioning.** There is no schema-version field on the thread,
  message, or summary record, and no documented ratchet direction. How is a
  breaking transcript-shape change intended to roll out, given the
  `next_sequence` precedent of pinning old threads to old behavior forever?
- **Idempotency-record lifecycle.** Who deletes
  `/threads/idempotency/<sha256>.json`? We found no sweeper, no TTL, and no
  cascade from `delete_thread`. Is unbounded growth accepted, or is a sweeper
  planned elsewhere?
- **Parent-delete cascade for subagent threads.** Child threads are siblings
  linked only at the run level. What is the intended behavior for a child's
  transcript when the parent thread is deleted, and is there any enumeration
  path from parent thread to child threads at all?
- **Retention ownership for transcripts.** Nothing enforces transcript
  retention. Is that intended to be a deployment concern (SQL-side policy on
  `root_filesystem_entries`), a future product feature, or a deliberate
  never-delete stance?
- **`RecoveryRequired` status.** `turn-persistence.md:91` and
  `Architecture.md:578-579` disagree on whether an expired lease is terminal.
  Which document reflects the current code path in every backend, and is
  `turn-persistence.md` simply stale?
- **Cross-scope thread movement.** `ensure_thread` rejects a scope/thread
  mismatch with `ThreadScopeMismatch` (`filesystem_service.rs:1320`), and scope
  is baked into the path. Is re-parenting a thread (new project, new owner)
  supported at all, or is it a copy-and-abandon operation?
- **Sequence gaps.** A reserved sequence whose message write then fails leaves a
  hole in the sequence space. Do readers, the ordered index, and summary-range
  validation all treat gaps as benign, and is that stated anywhere as a
  contract?
- **Multi-writer transcript ordering under steering.** With `Queued` messages
  and a single active run per `TurnScope`, what guarantees the model sees queued
  user messages in accept order after a busy period, given that
  `load_context_window` selects by sequence over the latest N?
