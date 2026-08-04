# Mastra: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-08-04. Mastra is source-available, so every
claim below cites a repo-root-relative `path:line` against the pinned commit
rather than using [observed]/[literal] evidence tags (those apply to
closed-source, black-box targets like `fx`, not here). Version-sensitive
claims were checked against these anchors:

- Source: local clone of `mastra-ai/mastra`, pinned at commit
  `9e1dad8f7b1cab2bb7ade90e5b7561f24577b88a`. All citations below are
  repo-root-relative paths within that clone.
- `packages/core/src/storage/domains/memory/base.ts` -- the abstract
  `MemoryStorage` domain interface (this dossier's centerpiece).
- `packages/core/src/storage/types.ts`, `packages/core/src/storage/constants.ts`,
  `packages/core/src/memory/types.ts`,
  `packages/core/src/agent/message-list/state/types.ts` -- thread/message
  type definitions and physical table schemas.
- `packages/core/src/storage/base.ts`, `packages/core/src/storage/retention.ts`,
  `packages/core/src/storage/workflow-snapshot.ts` -- composite-store,
  retention, and workflow-snapshot machinery.
- `packages/core/src/processors/memory/semantic-recall.ts`,
  `packages/core/src/processors/memory/message-history.ts`,
  `packages/core/src/processors/memory/working-memory.ts`,
  `packages/core/src/memory/memory.ts` -- the read/resume and derived-index
  paths.
- `packages/core/src/agent/agent.ts`, `packages/core/src/agent-controller/`
  (`agent-controller.ts`, `tools.ts`, `session.ts`) -- the two independent
  sub-agent/fork mechanisms.
- `packages/core/src/mastra/index.ts` -- ID generation.
- Backend adapters actually read for this dossier:
  `stores/pg/src/storage/domains/memory/index.ts` (3077 lines, read directly),
  `stores/libsql/src/storage/domains/memory/index.ts` (2738 lines),
  `stores/dynamodb/src/storage/domains/memory/index.ts` (1133 lines),
  `stores/mongodb/src/storage/domains/memory/index.ts` (2441 lines). These
  four were chosen to span the space of storage models: relational-with-real-
  transactions (pg), embedded-SQL-with-batch-not-transaction (libsql),
  single-table NoSQL with no multi-item transaction primitive at all
  (dynamodb), and document-store with topology-conditional transactions
  (mongodb). **Not read**: `stores/clickhouse`, `stores/cloudflare-d1`,
  `stores/convex`, `stores/dsql`, `stores/mssql`, `stores/mysql`,
  `stores/redis`, `stores/spanner`, `stores/upstash` -- their
  `domains/memory` implementations exist (confirmed by directory listing) but
  were not opened; claims about them are limited to what the shared abstract
  interface and error-message text state.

**License note**: the repo root is Apache-2.0
(`package.json`, `"license": "Apache-2.0"`), but `LICENSE.md` carves out
every `ee/` directory, which is instead governed by `ee/LICENSE` (the Mastra Enterprise
Edition license, a proprietary agreement with Kepler Software, Inc. required
for production use). Nothing in this dossier cites any path under `ee/`
(e.g. `packages/core/src/auth/ee/`, `packages/server/src/server/auth/ee/`);
every finding below comes from the Apache-2.0-licensed tree and is usable as
open-source precedent.

## The storage model

Mastra does not have one storage model -- it has at least three, layered
under one composition mechanism, and this dossier is scoped to the first.

**1. Thread + Message (the session transcript, this dossier's subject).**
The source of truth is a row set: one `StorageThreadType` row per
conversation thread and one row per message, held in whatever the backend
natively is (Postgres/MySQL/libSQL tables, a single DynamoDB table via
ElectroDB entities, MongoDB collections). There is no single append-only log
file anywhere in this path -- durability is "one row insert per message,"
not "one line appended to a stream." The physical column list is fixed by
`TABLE_SCHEMAS[TABLE_THREADS]` and `TABLE_SCHEMAS[TABLE_MESSAGES]`
(`packages/core/src/storage/constants.ts:661-668` and `:669-677`); every
backend is expected to reproduce this shape (see **The store interface**).

**2. Derived/rebuildable projections layered on top of the message rows.**
Semantic recall's vector index is fully derived: it is (re)built by
embedding message content and upserting into a vector store keyed by
message ID (`packages/core/src/processors/memory/semantic-recall.ts:649-657`),
and the index itself is created idempotently on first use
(`ensureVectorIndex`,
`packages/core/src/processors/memory/semantic-recall.ts:511-533`) -- losing the vector
index loses nothing durable, it can be rebuilt by re-embedding the message
table. This is the clearest rebuildable-cache example in the codebase.

**3. Two more "session-shaped" primitives that are *not* derived from
messages, and not each other:**
- **Observational Memory (OM)** is a separate, generation-versioned
  "reflection" record (`ObservationalMemoryRecord`,
  `packages/core/src/storage/types.ts:1164-1204+`) stored in its own table
  (`TABLE_OBSERVATIONAL_MEMORY = 'mastra_observational_memory'`,
  `packages/core/src/storage/constants.ts:14`, schema at
  `packages/core/src/storage/constants.ts:503`). It is authoritative, not
  a cache of the messages -- losing it loses the condensed memory, even
  though the raw messages that produced it are still present.
- **Workflow run state** (`WorkflowRunState`,
  `packages/core/src/storage/workflow-snapshot.ts:41-56`) is a single mutable
  JSON document per run (`context, activePaths, suspendedPaths, status,
  runId`, ...), stored and rewritten as a whole blob -- the polar opposite of
  the messages table's insert-per-row model. `createEmptyWorkflowSnapshot()`
  (`packages/core/src/storage/workflow-snapshot.ts:41`) and
  `mergeWorkflowStepResult()`
  (`packages/core/src/storage/workflow-snapshot.ts:57`) both operate on the entire document, not a
  delta log.
- A fourth, higher-level notion -- `harness` domain `SessionRecord` -- wraps a
  thread with session-level metadata (mode, model, pending approvals). It is
  covered under **Subagents and nested sessions** because its most notable
  feature for this dossier is an apparently-unused parent-child field.

**Best-fit conceptual model**: session-as-row-set (thread row + append-
style message rows) for the piece this dossier is scoped to, with a
completely different session-as-mutable-document model for workflow runs
existing side by side in the same product. *(Inference: Mastra's own
internal primitives already disagree on whether "session state" should be
append-oriented or a single overwritten document -- this is itself evidence
that both shapes are legitimate, are chosen per-primitive based on access
pattern, not fixed by one platform-wide session abstraction.)*

## Keying and identity

- A thread is addressed by `StorageThreadType.id`, always scoped to a
  `resourceId` (`packages/core/src/memory/types.ts:39-46`):
  ```ts
  export type StorageThreadType = {
    id: string;
    title?: string;
    resourceId: string;
    createdAt: Date;
    updatedAt: Date;
    metadata?: Record<string, unknown>;
  };
  ```
  `resourceId` is the tenant/user/agent-owner axis; `id` is the conversation
  axis. Listing is filtered by `resourceId` (and optionally `metadata`) via
  `StorageListThreadsInput.filter` (`packages/core/src/storage/types.ts:183-193`),
  never by scanning all threads platform-wide by default.
- **ID minting**: the default is a plain `crypto.randomUUID()` returned by
  `Mastra.generateId()` (`packages/core/src/mastra/index.ts:1128-1144`,
  specifically the fallback at line 1143). This is pluggable -- a caller can
  register a custom `#idGenerator(context: IdGeneratorContext)` -- but nothing
  in core enforces or assumes an ordering-encoding scheme (no UUIDv7 by
  default); ordering is carried entirely by the separate `createdAt` column,
  not by the ID.
- **Sub-agent/forked thread IDs use two unrelated schemes** (detailed fully
  under **Subagents and nested sessions**):
  1. The agent-to-agent delegation path mints an ID by string concatenation:
     `` `${inputData.threadId}-${randomUUID()}` `` and
     `` `${inputData.resourceId}-${agentName}` ``
     (`packages/core/src/agent/agent.ts:4716-4731`). Nothing downstream
     parses this string back apart -- a grep across `packages/core/src` for
     any `startsWith`/pattern-based reconstruction of thread hierarchy from
     this ID shape returns no hits.
  2. The `agent-controller`'s `subagent` tool fork path instead calls
     `MemoryStorage.cloneThread()`, which mints its own independent
     `crypto.randomUUID()` for the child thread unless a caller supplies
     `newThreadId` (`packages/core/src/storage/types.ts:218-219`); parentage
     is recorded in `thread.metadata`, not in the ID (see below).
- **Listing scope**: never cross-resource by default. The
  `agent-controller`'s `listThreads` also does an *in-process* filter to hide
  forked-subagent threads (`packages/core/src/agent-controller/agent-controller.ts:1005-1012`):
  it fetches the full unfiltered page from storage
  (`memoryStorage.listThreads({filter, perPage: false})`, line 1005) and
  then array-filters on `metadata.forkedSubagent !== true` (line 1011) -- the
  storage layer itself has no predicate for "exclude forks," so this
  filtering does not benefit from any index and re-reads the full result set
  every call. *(Inference.)*
- **Relocation/rename**: not applicable in the sense the prompt means for
  filesystem-rooted CLI agents -- Mastra threads are pure database rows keyed
  by `id`/`resourceId`, not paths tied to a working directory or worktree.
  `updateThread` (`packages/core/src/storage/domains/memory/base.ts:63-71`)
  is the rename path (title/metadata only; `id` and `resourceId` are
  immutable post-creation in the abstract contract).

## The store interface

Mastra's storage layer is layered: a base `StorageDomain` class
(`packages/core/src/storage/domains/base.ts`, not read in detail -- out of
scope) is specialized per concern (memory, agents, workflows, harness,
observability, scores, thread-state, ...), and each concern is implemented
per backend under `stores/*/src/storage/domains/<domain>/`. `MemoryStorage`
(`packages/core/src/storage/domains/memory/base.ts:38-456`) is the
thread/message contract -- the centerpiece of this dossier -- reproduced
verbatim below (JSDoc trimmed for space; every signature is exact):

```ts
// packages/core/src/storage/domains/memory/base.ts
export abstract class MemoryStorage extends StorageDomain {
  readonly supportsObservationalMemory?: boolean = false;   // :44

  // --- Threads: all four REQUIRED (abstract, no default) ---
  abstract getThreadById({ threadId, resourceId }: {
    threadId: string; resourceId?: string;
  }): Promise<StorageThreadType | null>;                    // :53-59

  abstract saveThread({ thread }: {
    thread: StorageThreadType;
  }): Promise<StorageThreadType>;                            // :61

  abstract updateThread({ id, title, metadata }: {
    id: string; title: string; metadata: Record<string, unknown>;
  }): Promise<StorageThreadType>;                             // :63-71

  abstract deleteThread({ threadId }: { threadId: string }): Promise<void>;  // :73

  // --- Messages: mostly required, two opt-in ---
  abstract listMessages(args: StorageListMessagesInput):
    Promise<StorageListMessagesOutput>;                       // :75

  // OPTIONAL -- default throws. Backend opts in by overriding.
  async listMessagesByResourceId(_args: StorageListMessagesByResourceIdInput):
    Promise<StorageListMessagesOutput> {
    throw new Error(
      `Resource-scoped message listing is not implemented by this storage adapter (${this.constructor.name}). ` +
      `Use an adapter that supports Observational Memory (pg, libsql, mongodb, convex) or disable observational memory.`,
    );
  }                                                            // :84-89

  abstract listMessagesById({ messageIds }: { messageIds: string[] }):
    Promise<{ messages: MastraDBMessage[] }>;                  // :91

  abstract saveMessages(args: { messages: MastraDBMessage[] }):
    Promise<{ messages: MastraDBMessage[] }>;                  // :93

  abstract updateMessages(args: {
    messages: (Partial<Omit<MastraDBMessage, 'createdAt'>> & {
      id: string;
      content?: { metadata?: MastraMessageContentV2['metadata']; content?: MastraMessageContentV2['content'] };
    })[];
  }): Promise<MastraDBMessage[]>;                              // :95-100

  // OPTIONAL -- default throws.
  async deleteMessages(_messageIds: string[]): Promise<void> {
    throw new Error(
      `Message deletion is not supported by this storage adapter (${this.constructor.name}). ` +
      `The deleteMessages method needs to be implemented in the storage adapter.`,
    );
  }                                                            // :102-107

  abstract listThreads(args: StorageListThreadsInput):
    Promise<StorageListThreadsOutput>;                         // :118

  // OPTIONAL -- default throws. pg/libsql/mongodb/mysql/redis/upstash implement it (grep-confirmed); dynamodb does not.
  async cloneThread(_args: StorageCloneThreadInput): Promise<StorageCloneThreadOutput> {
    throw new Error(
      `Thread cloning is not implemented by this storage adapter (${this.constructor.name}). ` +
      `The cloneThread method needs to be implemented in the storage adapter.`,
    );
  }                                                            // :127-132

  // Resource (working-memory) methods: default-throwing, but the throw text
  // itself says "This is likely a bug - all Mastra storage adapters should
  // implement resource support." (:137, repeated verbatim at :145 and :157)
  async getResourceById / saveResource / updateResource ... { throw ... }   // :134-160

  protected parseOrderBy(...) { ... }                          // :162-173

  // 16 Observational Memory methods -- ALL default-throwing unless overridden:
  // getObservationalMemory :183, getObservationalMemoryHistory :194,
  // initializeObservationalMemory :207, updateActiveObservations :215,
  // updateBufferedObservations :228, swapBufferedToActive :242,
  // createReflectionGeneration :253, updateBufferedReflection :261,
  // swapBufferedReflectionToActive :270, setReflectingFlag :279,
  // setObservingFlag :286, setBufferingObservationFlag :297,
  // setBufferingReflectionFlag :305, insertObservationalMemoryRecord :313,
  // clearObservationalMemory :321, setPendingMessageTokens :330,
  // updateObservationalMemoryConfig :338

  protected deepMergeConfig / validateMetadataKeys / validatePagination /
    validatePaginationInput ...                                 // :346-455
}
```

**Required vs optional, precisely**: `getThreadById`, `saveThread`,
`updateThread`, `deleteThread`, `listMessages`, `listMessagesById`,
`saveMessages`, `updateMessages`, `listThreads` are `abstract` -- every
backend must implement all nine or TypeScript will not compile it.
`listMessagesByResourceId`, `deleteMessages`, `cloneThread`,
`getResourceById`/`saveResource`/`updateResource`, and all 16 Observational
Memory methods are concrete methods on the base class that simply `throw`
-- a backend "supports" them purely by choosing to override the method; there
is no capability flag except `supportsObservationalMemory` (line 44, a
plain boolean the OM subsystem reads to decide whether to attempt OM calls
at all) -- the other optional methods are discovered only by calling them
and catching the throw, i.e., failure is the capability-detection
mechanism. *(Inference: this is a weaker capability model than a declared
feature-flag object -- a caller cannot introspect what a given store instance
supports without a trial call or reading its class.)*

Message rows at the physical/storage-schema level are a distinct, flatter
shape from the domain-level `MastraDBMessage` -- see **Entry/message
structure and versioning**.

## Write and append path (ordering, durability, concurrency, delivery)

`saveMessages` is the append primitive (there is no separate "append single
message" method -- every save is a batch of one or more). Behavior diverges
sharply across the four backends read for this dossier, even though all
four implement the exact same abstract signature:

**Postgres -- real, all-or-nothing transaction.** `saveMessages`
(`stores/pg/src/storage/domains/memory/index.ts:1355`) deduplicates by ID
first (`dedupeMessagesForSave()`,
`stores/pg/src/storage/domains/memory/index.ts:139-156`, which on a
duplicate ID keeps the *existing* record's `createdAt` rather than the
incoming one -- so re-saving an already-stored message id cannot rewrite
its timestamp), then wraps a chunked `INSERT ... ON CONFLICT (id) DO
UPDATE` (chunked by `MAX_MESSAGES_PER_INSERT`,
`stores/pg/src/storage/domains/memory/index.ts:1402-1438`) **and** the
thread's `updatedAt`/`updatedAtZ` bump
(`stores/pg/src/storage/domains/memory/index.ts:1440-1451`) inside one
`this.#db.client.tx(async t => {...})` block
(`stores/pg/src/storage/domains/memory/index.ts:1401-1452`). If the process
dies mid-write, either all of it lands or none of it does. `deleteThread`
(`stores/pg/src/storage/domains/memory/index.ts:765-803`) is likewise one
transaction: delete messages, scan `pg_tables` for `memory_messages%`
vector-index tables and purge rows tagged with that `thread_id`
(`stores/pg/src/storage/domains/memory/index.ts:772-786`), then delete the
thread row (`stores/pg/src/storage/domains/memory/index.ts:788`) -- all
inside `client.tx` (`stores/pg/src/storage/domains/memory/index.ts:769`).

**libSQL -- batched but NOT fully atomic.** `saveMessages`
(`stores/libsql/src/storage/domains/memory/index.ts:725`) builds one
`INSERT ... ON CONFLICT(id) DO UPDATE` statement per message
(`stores/libsql/src/storage/domains/memory/index.ts:747-767`) and executes
them via `this.#client.batch(batch, 'write')` in chunks of 50
(`BATCH_SIZE`, `stores/libsql/src/storage/domains/memory/index.ts:776,783-788`)
-- but the thread's `updatedAt` bump is pushed onto the *same*
`batchStatements` array
(`stores/libsql/src/storage/domains/memory/index.ts:770-773`) and then
explicitly sliced off and executed **separately**, outside any batch, via
a lone `this.#client.execute(...)` call
(`stores/libsql/src/storage/domains/memory/index.ts:791-792`). A crash
between the message batch(es) and this final `execute` leaves messages
durably saved with a stale `thread.updatedAt`. Additionally, when a save
exceeds 50 messages, it spans *multiple* `.batch()` calls -- each is its own
atomic unit, but the whole `saveMessages` invocation is not: a crash after
batch 1 but before batch 2 leaves a saved-but-partial message set with no
transaction rolling it back. `deleteThread`
(`stores/libsql/src/storage/domains/memory/index.ts:1356-1381`) is explicit
about *not* using a transaction, and says why in a code comment: "Not
using a transaction to avoid `SQLITE_BUSY` errors when multiple
`deleteThread` calls run concurrently... orphaned messages (if thread
delete fails) would be cleaned up on next delete attempt"
(`stores/libsql/src/storage/domains/memory/index.ts:1358-1361`) -- a
documented, deliberate atomicity-for-availability trade, with cleanup left
informal ("next delete attempt," which is never guaranteed to happen).

**DynamoDB -- no multi-item transaction at all; manual, fallible rollback.**
`saveMessages` (`stores/dynamodb/src/storage/domains/memory/index.ts:599`)
writes messages **sequentially**, one ElectroDB `.put().go()` call per
message (`stores/dynamodb/src/storage/domains/memory/index.ts:644`), and
on any failure attempts a compensating rollback by deleting every message
already written in that call
(`stores/dynamodb/src/storage/domains/memory/index.ts:647-658`) -- but that
rollback loop itself just logs and swallows its own failures
(`stores/dynamodb/src/storage/domains/memory/index.ts:651-655`: `catch
(rollbackError) { this.logger.error(...) }`, no retry, no re-throw of the
rollback failure). The thread's `updatedAt` bump happens after the entire
message loop (`stores/dynamodb/src/storage/domains/memory/index.ts:663-668`)
and is not covered by the rollback at all -- if it throws, already-written
messages stay written. `deleteThread`
(`stores/dynamodb/src/storage/domains/memory/index.ts:281-320`) similarly
has zero compensating logic: it lists all messages (`perPage: false`,
`stores/dynamodb/src/storage/domains/memory/index.ts:287`), deletes them
in `Promise.all` batches of 25
(`stores/dynamodb/src/storage/domains/memory/index.ts:290-304`, DynamoDB's
`BatchWriteItem` limit), then deletes the thread row
(`stores/dynamodb/src/storage/domains/memory/index.ts:308`) -- if the
thread-row delete fails after all messages are gone, nothing detects or
repairs the now-orphaned-but-message-less thread. `cloneThread` is not
implemented for this backend (no `async cloneThread` found in the file),
so the abstract base's default-throwing implementation is what callers get
(`packages/core/src/storage/domains/memory/base.ts:127-132`).

**MongoDB -- atomicity conditional on cluster topology, and it degrades
silently.** `saveMessages`
(`stores/mongodb/src/storage/domains/memory/index.ts:695`) does a
`bulkWrite` of per-message `updateOne`+`upsert` operations
(`stores/mongodb/src/storage/domains/memory/index.ts:721-740`) plus each
touched thread's `updatedAt`, wrapped in
`this.#connector.withTransaction(async session => {...})`
(`stores/mongodb/src/storage/domains/memory/index.ts:750-755`), with an
explicit comment: "Operations are sequential because a transaction session
is not concurrency-safe; on a standalone server this degrades to the same
sequential best-effort behavior"
(`stores/mongodb/src/storage/domains/memory/index.ts:746-749`).
`withTransaction()`
(`stores/mongodb/src/storage/connectors/MongoDBConnector.ts:123-138`) probes
`supportsTransactions()` (cached after first check, line 124) and, if
unsupported (i.e. a standalone `mongod`, not a replica set), **runs the
callback with `session=undefined`** (line 126) -- no error, no warning
surfaced to the caller, just a quiet loss of atomicity. `deleteThread`
(`stores/mongodb/src/storage/domains/memory/index.ts:1212-1237`) goes
further and *never* uses a transaction, with a code comment explaining
exactly why: a transactional `deleteMany` is capped by MongoDB's
`transactionLifetimeLimitSeconds` (60s default) and must hold every pending
delete in memory until commit, so a sufficiently large thread would "abort
and become permanently undeletable"; a plain `deleteMany` "commits
incrementally and always completes"
(`stores/mongodb/src/storage/domains/memory/index.ts:1214-1222`, quoted in
full -- this is the single clearest piece of divergence evidence in the
whole corpus: an adapter deliberately giving up atomicity because the
transactional alternative has a hard ceiling the untransacted path does
not).

**Ordering tiebreak also silently diverges.** Postgres and libSQL both
consistently order message reads by `(createdAt, id)` as a two-column
tiebreak: pg at `stores/pg/src/storage/domains/memory/index.ts:887,903,922`;
libsql at `stores/libsql/src/storage/domains/memory/index.ts:307-308,319-320,332`.
MongoDB is *inconsistent with itself*: the before/after window pair in one
method carries the `id` tiebreak in both directions, `{createdAt: -1, id: -1}`
for the messages at or before the target and `{createdAt: 1, id: 1}` for the
ones after it
(`stores/mongodb/src/storage/domains/memory/index.ts:289,298`), while at least
one other query sorts by `{createdAt: 1}` alone, with no `id` tiebreak
(`stores/mongodb/src/storage/domains/memory/index.ts:1322`).
Because `createdAt` values can collide
(multiple messages saved in the same batch share very close or identical
timestamps depending on clock resolution), two messages can come back in a
different relative order depending which code path reads them -- a real,
citable, backend-internal ordering inconsistency, not just a cross-backend
one. DynamoDB has no natural row order at all; its base entity key has none,
so ordering is entirely a property of whichever GSI/sort key the query
uses, and pagination/offset is emulated in application code rather than
being a native database feature (`stores/dynamodb/src/storage/domains/memory/index.ts:376-380`
computes an `offset`/`perPage` pair that the query layer then has to
reconcile against DynamoDB's cursor-based `LastEvaluatedKey` model --
confirmed by the presence of `calculatePagination`/`normalizePerPage`
helpers at that call site (`stores/dynamodb/src/storage/domains/memory/index.ts:376,378`),
though the O(N) re-read cost this can imply for deep pages was reported by
an earlier deep-read of this file and was not independently re-derived
line-by-line in this final pass; flagged as *lower-confidence, inference*
rather than a directly quoted cost figure, see **Open questions**).

**Concurrency/expected-version**: none of the abstract signatures
(`saveMessages`, `updateThread`, `saveThread`) accept an expected-version or
compare-and-swap precondition
(`packages/core/src/storage/domains/memory/base.ts:61-71,93`). Every
backend observed resolves same-ID conflicts with a last-write-wins upsert
(`ON CONFLICT ... DO UPDATE` in pg/libsql, ElectroDB `upsert`/`put` in
dynamodb, Mongo `updateOne({upsert:true})`). There is no optimistic-locking
mechanism anywhere in this path; concurrent writers to the same thread
race, and the last write physically committed wins. *(Inference: the
product assumes effectively single-writer-per-thread in practice, since
there is no protocol to detect or reject a stale write.)*

**Delivery semantics**: no retry/outbox/at-least-once framework exists at
the `MemoryStorage` boundary itself -- it is called once per turn by the
in-process agent loop. The only idempotence guard is the primary-key
upsert on message `id` (so a caller-side retry that resends the same
message object is safe by construction, not because of any queue
deduplication layer).

## Read and resume path

Resume is **not** a full-log replay by default -- it is a bounded, eager
"last N messages" reload executed fresh on every turn. The
`MessageHistory` input processor
(`packages/core/src/processors/memory/message-history.ts:113-119`) issues:

```ts
const result = await this.storage.listMessages({
  threadId, resourceId, page: 0,
  perPage: this.lastMessages,
  orderBy: { field: 'createdAt', direction: 'DESC' },
});
```

then reverses the DESC page back to chronological order
(`packages/core/src/processors/memory/message-history.ts:133`) before
handing it to the model. `lastMessages` defaults to `10`
(`packages/core/src/memory/memory.ts:83`) and is per-`Memory`-config; it
reads the durable store directly on every turn -- there is no separate
local cache read first. This is wired in only when
`effectiveConfig.lastMessages` is truthy and neither a user-supplied
`message-history` processor nor Observational Memory is already handling
message loading (`packages/core/src/memory/memory.ts:760-786`) -- when OM is
enabled, it "handles its own message loading and saving" instead
(`packages/core/src/memory/memory.ts:773-779`), i.e., the eager
last-N-messages window is bypassed entirely in favor of OM's condensed
generation record.

Independently, when semantic recall is configured, it performs a **second**
read pass: `SemanticRecall.performSemanticSearch()`
(`packages/core/src/processors/memory/semantic-recall.ts:385-455`) queries
the vector index (filtered by `thread_id`/`resource_id`) for the most
relevant *older* messages, then re-fetches the **full** message content
from the durable store by ID rather than trusting the vector metadata
payload (confirmed by the vector metadata shape at
`packages/core/src/processors/memory/semantic-recall.ts:649-657` carrying only
`{message_id, thread_id, resource_id, role, content, created_at}` -- a
denormalized summary, not the canonical row) -- so semantic recall never
uses the vector store as the source of truth for message content, only as
an index into IDs.

There is entry-level pagination (`page`/`perPage` on `listMessages`,
`packages/core/src/storage/types.ts:73-131`) and every backend enforces it;
there is no unbounded "load the whole thread" resume path unless a caller
explicitly passes `perPage: false`. Postgres additionally supports a
context-expansion pagination mode via `include: [{id, withPreviousMessages,
withNextMessages}]` (`packages/core/src/storage/types.ts:73-79`), implemented with a
cursor-based approach that explicitly replaced an earlier `ROW_NUMBER()`
window-function approach for performance reasons, per a code comment citing
a production GitHub issue (`stores/pg/src/storage/domains/memory/index.ts:805-809`,
"This replaces the previous `ROW_NUMBER()` approach which caused severe
performance issues on large tables (see GitHub issue #11150)").

## Listing, summaries, and search

`listThreads` (`packages/core/src/storage/domains/memory/base.ts:118`,
input/output at `packages/core/src/storage/types.ts:168-198`) is the enumeration path: filter
by `resourceId` and/or shallow `metadata` key-value pairs (AND logic),
ordered by `createdAt`/`updatedAt`, paginated (`perPage` default 100,
`packages/core/src/storage/types.ts:171-172`). There is no separately-maintained summary
sidecar/read-model for threads distinct from the thread row itself -- the
thread row (`title`, `metadata`, timestamps) *is* the list-view record; a
picker reads the same table it would read for resume, just without
messages.

Search is a genuinely separate indexed subsystem only for semantic recall:
a vector index named via `getDefaultIndexName()`
(`packages/core/src/processors/memory/semantic-recall.ts:496-510`, pattern
`` `mastra_memory_${sanitizedModel}` ``, truncated to 63 chars for backend
name-length limits) is created idempotently through `ensureVectorIndex()`
(`packages/core/src/processors/memory/semantic-recall.ts:511-533`), which calls
`this.vector.createIndex(...)`
(`packages/core/src/processors/memory/semantic-recall.ts:520`) guarded by an
in-memory dimension-validation
cache so repeated calls are cheap no-ops once the index exists. It is kept
consistent with the message log at write time: `processOutputResult()`
(`packages/core/src/processors/memory/semantic-recall.ts:534-657`) embeds new user/assistant messages (skipping
system messages) and immediately `vector.upsert(...)`s them
(`packages/core/src/processors/memory/semantic-recall.ts:649-657`) -- there is no separate background indexing
job; the index is updated synchronously in the same turn that produces the
message. If the vector store is lost or reset, it can be fully rebuilt by
re-embedding the message table (this was not observed as an actual
"rebuild" code path -- no explicit reindex-from-message-table function was
found in the surveyed files; this is an *inference* from the fact that the
index is keyed purely off message IDs and content that already live
durably elsewhere, not a confirmed reindex utility. Flagged under **Open
questions**).

Observational Memory has its own, unrelated lookup path: records are
fetched by `lookupKey` ordered by `generationCount DESC LIMIT 1`
(e.g. `stores/pg/src/storage/domains/memory/index.ts:1997`,
`stores/libsql/src/storage/domains/memory/index.ts:1635`) -- a "most recent
generation" query, not a search index.

## Entry/message structure and versioning

Two distinct message shapes exist, one at the storage-row level and one at
the domain level, and the store's job is largely translating between them.

**Storage-row shape** (`StorageMessageType`,
`packages/core/src/storage/types.ts:262-270`):
```ts
export type StorageMessageType = {
  id: string;
  thread_id: string;
  content: string;          // opaque serialized string at this layer
  role: string;
  type: string;
  createdAt: Date;
  resourceId: string | null;
};
```
and the matching physical column schema
(`packages/core/src/storage/constants.ts:669-677`) types `content` as
`'text'` -- i.e., every backend observed (pg, libsql, dynamodb, mongodb)
`JSON.stringify()`s the domain-level content object before writing and
`JSON.parse()`s it back on read (e.g.
`stores/pg/src/storage/domains/memory/index.ts:1411,1454-1459`;
`stores/libsql/src/storage/domains/memory/index.ts:760`;
`stores/dynamodb/src/storage/domains/memory/index.ts:622`;
`stores/mongodb/src/storage/domains/memory/index.ts:728`). The store treats
`content` as opaque bytes; it does not query into the JSON structure at the
SQL/query layer (metadata filtering, where supported, operates on a
separately-extracted shallow scalar map -- `StorageMetadataFilter`,
`packages/core/src/storage/types.ts:66-68` -- not on arbitrary JSON paths inside `content`).

**Domain-level shape** (`MastraDBMessage`,
`packages/core/src/agent/message-list/state/types.ts:107-109`):
```ts
type MastraMessageShared = {                              // :16-23
  id: string;
  role: 'user' | 'assistant' | 'system' | 'signal';
  createdAt: Date;
  threadId?: string;
  resourceId?: string;
  type?: string;
};
export type MastraDBMessage = MastraMessageShared & {
  content: MastraMessageContentV2;                          // :107-109
};
export type MastraMessageContentV2 = {                      // :94-104
  format: 2;                            // format 2 === UIMessage in AI SDK v4
  parts: MastraMessagePart[];
  experimental_attachments?: UIMessageV4['experimental_attachments'];
  content?: UIMessageV4['content'];
  toolInvocations?: UIMessageV4['toolInvocations'];
  reasoning?: UIMessageV4['reasoning'];
  annotations?: UIMessageV4['annotations'];
  metadata?: Record<string, unknown>;
  providerMetadata?: MastraProviderMetadata;
};
```
Message *kind* is distinguished by `role` (`'user'|'assistant'|'system'|
'signal'`) at the envelope level, and within `content.parts` by a
discriminated `type` tag on each `MastraMessagePart` -- `MastraToolInvocationPart`
(`packages/core/src/agent/message-list/state/types.ts:52-59`),
`MastraSourceDocumentPart`
(`packages/core/src/agent/message-list/state/types.ts:61-69`),
`MastraSourceUrlPart`
(`packages/core/src/agent/message-list/state/types.ts:71-74`),
`MastraStepStartPart`
(`packages/core/src/agent/message-list/state/types.ts:31-34`), plus the
inherited AI-SDK-v4 UI part types. Ordering into a thread is purely by the
envelope's `createdAt` (plus the backend-specific `id` tiebreak discussed
above) -- there is no `parentMessageId`/chain-link field anywhere in either
shape; a thread's message order is entirely a property of the row set, not
of any in-band linking field.

**Schema evolution / versioning**: there is a `type: 'v1'|'v2'` tag stored
per message row (seen as `message.type || 'v2'` on every backend's insert,
e.g. `stores/pg/src/storage/domains/memory/index.ts:1415`) and a `format: 2`
tag inside `MastraMessageContentV2` itself
(`packages/core/src/agent/message-list/state/types.ts:95`). A distinct
legacy shape, `MastraMessageV1`
(`packages/core/src/memory/types.ts:21-32`, also redeclared at
`packages/core/src/agent/message-list/state/types.ts:112-123`), is not
stored anymore but is produced on read for
legacy consumers by a pure, non-persisted conversion function,
`convertToV1Messages()`
(`packages/core/src/agent/message-list/prompt/convert-to-mastra-v1.ts:58`),
which includes ID-splitting logic (a `__split-N` suffix pattern,
`packages/core/src/agent/message-list/prompt/convert-to-mastra-v1.ts:16`) for V2 messages that must be broken into
multiple V1 messages during downgrade. This is a read-time compatibility
shim, not a migration -- the durable row keeps its native v2 shape and the
v1 view is synthesized on demand.

**Per-backend physical migrations** are real and additive. Postgres:
`OM_MIGRATION_COLUMNS`
(`stores/pg/src/storage/domains/memory/index.ts:40-56`) is an explicit list
of 15 columns added to the Observational Memory table for backward
compatibility, applied via `alterTable({tableName: OM_TABLE, ifNotExists:
OM_MIGRATION_COLUMNS})`
(`stores/pg/src/storage/domains/memory/index.ts:211-215`) and a similar
`alterTable({tableName: TABLE_MESSAGES, ifNotExists: ['resourceId']})`
(`stores/pg/src/storage/domains/memory/index.ts:217-221`) inside `init()`
(`stores/pg/src/storage/domains/memory/index.ts:192-232`) -- i.e., an
existing deployed table gains the `resourceId` column non-destructively on
next boot. A code comment at the top of that same `init()` documents a real
production incident this migration path is protecting against: a
dynamic-import-based schema guard that esbuild's bundling broke, tracked as
issue `#18298` (`stores/pg/src/storage/domains/memory/index.ts:197-202`,
"Don't switch this to `await import(...)`: that used to deadlock `mastra
build` output... the cycle never resolves when storage initializes during
module evaluation (`#18298`)"). LibSQL runs its own migration path via
SQLite's `PRAGMA
table_info(...)` introspection
(`stores/libsql/src/storage/factory-storage.ts:504,539`) followed by
per-missing-column `ALTER TABLE ... ADD COLUMN`
(`stores/libsql/src/storage/factory-storage.ts:543`), and for changes that cannot be expressed as an
additive `ALTER TABLE` (SQLite's DDL is limited), a shadow-table-and-rename
pattern (`stores/libsql/src/storage/factory-storage.ts:525`, `` `ALTER TABLE "${shadow}" RENAME TO
"${schema.name}"` ``). There is no single global schema-version integer
anywhere observed; each backend's migration is scoped to detecting missing
columns/tables at `init()` time, not to a monotonic version counter.

## Compaction and history management

No compaction of the *durable* message row set was found -- messages are
not truncated, summarized-in-place, or merged by the storage layer itself.
What shrinks the model-visible context window is upstream of storage:
`MessageHistory`'s `lastMessages` cap
(`packages/core/src/memory/memory.ts:83`, default 10) bounds how much of
the durable log is *read* per turn, and Observational Memory
(`ObservationalMemoryRecord.activeObservations`,
`packages/core/src/storage/types.ts:1198`) is a separately-durable
condensed *reflection* of history that can substitute for reading raw
messages at all (`packages/core/src/memory/memory.ts:773-779`) -- but it is
itself a first-class stored record with its own generation history
(`getObservationalMemoryHistory`,
`packages/core/src/storage/domains/memory/base.ts:194-201`, "returns
records in reverse chronological order"), not a compaction marker inside
the message table. There is no evidence of an in-place rewrite or
truncation marker written into `mastra_messages` itself; the raw log simply
keeps growing until retention (below) removes rows by age.

## Rewind, checkpoints, and fork

There is no retroactive "rewind/undo" primitive over the message log in
the surveyed anchors -- `updateMessages`
(`packages/core/src/storage/domains/memory/base.ts:95-100`) supports
in-place *edits* to specific message rows (see next paragraph) but nothing
resembling a rewind-to-checkpoint or branch-from-turn-N operation over the
raw thread. The closest thing to "fork" is `cloneThread`
(`packages/core/src/storage/domains/memory/base.ts:127-132`, input/output at
`packages/core/src/storage/types.ts:215-252`):
```ts
export type StorageCloneThreadInput = {
  sourceThreadId: string;
  newThreadId?: string;
  resourceId?: string;
  title?: string;
  metadata?: Record<string, unknown>;
  options?: {
    messageLimit?: number;
    messageFilter?: { startDate?: Date; endDate?: Date; messageIds?: string[] };
  };
};
export type StorageCloneThreadOutput = {
  thread: StorageThreadType;
  clonedMessages: MastraDBMessage[];
  messageIdMap?: Record<string, string>;   // used for OM remapping
};
```
This is copy-plus-lineage, not a shared-prefix reference: the implementations
read (pg `stores/pg/src/storage/domains/memory/index.ts:1745`, libsql
`stores/libsql/src/storage/domains/memory/index.ts:1383`, mongodb) each
physically duplicate the selected messages into new rows under a new thread
ID, and libsql's implementation additionally supports filtering by
`messageLimit`/date-range at clone time
(`stores/libsql/src/storage/domains/memory/index.ts:1413-1447`). Lineage
metadata is left to the *caller's own convention* rather than a fixed schema
column -- `packages/core/src/storage/types.ts:203-210` defines one convention
(`ThreadCloneMetadata { sourceThreadId, clonedAt, lastMessageId }`) but the
`agent-controller`'s fork tool independently invents a *different* metadata
shape for the same purpose (`{forkedSubagent: true, parentThreadId}`,
`packages/core/src/agent-controller/agent-controller.ts:1855-1858`) -- two
different ad hoc lineage-tagging conventions coexist in the same codebase,
both stored in the same opaque `thread.metadata` JSON column
(`packages/core/src/storage/constants.ts:663`, no first-class
`parentThreadId` column exists in `TABLE_THREADS`). *(Noted as inference:
this suggests Mastra has not settled on one canonical "this thread came
from that thread" schema field.)*

**Messages are not purely append-only.** `updateMessages`
(`packages/core/src/storage/domains/memory/base.ts:95-100`,
required/abstract) supports true in-place mutation. LibSQL's implementation
(`stores/libsql/src/storage/domains/memory/index.ts:809-913`) merges rather
than replaces the `content` field on update -- it deep-merges
`content.metadata` from the existing row with the incoming update
(`stores/libsql/src/storage/domains/memory/index.ts:854-868`) before
issuing an `UPDATE ... WHERE id = ?`. There is
no soft-delete/tombstone convention for messages at this layer; where
`deleteMessages` is implemented it is a hard `DELETE`.

No file-state/environment checkpoint concept tied to individual turns was
found anywhere in `packages/core/src/storage` (out of scope for a
conversational-memory store; Mastra's file/workspace-state concerns, where
present, live in a different subsystem not explored here -- flagged under
**Open questions** rather than asserted absent).

## Subagents and nested sessions

Mastra has **three independent, mutually-unaware mechanisms** that all
touch "child session" concerns, at three different layers of the stack.
This divergence -- not any single clean design -- is the most important
finding in this section.

**1. Cosmetic thread-ID concatenation (agent-to-agent delegation path).**
`packages/core/src/agent/agent.ts:4716-4731`:
```ts
const subAgentThreadId = inputData.threadId
  ? `${inputData.threadId}-${randomUUID()}`
  : context?.mastra?.generateId({ idType: 'thread', source: 'agent', entityId: agentName, resourceId }) || randomUUID();
const subAgentResourceId = inputData.resourceId
  ? `${inputData.resourceId}-${agentName}`
  : context?.mastra?.generateId({ idType: 'generic', source: 'agent', entityId: agentName }) || `${slugify.default(this.id)}-${agentName}`;
```
The child thread is a fully independent row with no schema-level link back
to the parent -- the parent-child relationship exists only in the shape of
the ID string. A grep of `packages/core/src` for any code that reconstructs
hierarchy from this pattern (`startsWith`, prefix matching, etc.) returns no
hits: it appears to be write-only metadata, informative to a human reading
IDs in a debugger, not consumed by any code path.

**2. Real thread-cloning fork (the `agent-controller`'s `subagent` tool).**
This is a durable, storage-layer mechanism, not cosmetic. When the
`subagent` tool is invoked with `forked: true`
(`packages/core/src/agent-controller/tools.ts:150-235`), it calls
`opts.cloneThreadForFork`, which is wired in
`packages/core/src/agent-controller/agent-controller.ts:1848-1861`:
```ts
cloneThreadForFork: hasMemory
  ? async ({ sourceThreadId, resourceId, title }) => {
      const memory = await this.resolveMemory(session);
      const result = await memory.cloneThread({
        sourceThreadId,
        resourceId: resourceId ?? session.identity.getResourceId(),
        title,
        metadata: { forkedSubagent: true, parentThreadId: sourceThreadId },
      });
      return { id: result.thread.id, resourceId: result.thread.resourceId };
    }
  : undefined,
```
This genuinely calls the storage-layer `MemoryStorage.cloneThread()`
(physically copying messages into a new thread row -- see **Rewind,
checkpoints, and fork**), and the parent link (`parentThreadId`) plus a
`forkedSubagent: true` tag are written into the *cloned* thread's opaque
`metadata` JSON column (not a first-class schema column -- `TABLE_THREADS`
has no `parentThreadId` column,
`packages/core/src/storage/constants.ts:661-668`). This tag is read back in
exactly one place: `listThreads()`'s default filter
(`packages/core/src/agent-controller/agent-controller.ts:1005-1012`) checks `metadata?.forkedSubagent !== true`
to hide fork threads from normal thread pickers unless
`includeForkedSubagents` is explicitly requested -- confirmed by a grep
across `packages/core/src` for `forkedSubagent`/`parentThreadId`, which
returns only these two files (`agent-controller.ts`, and one more use of
`forkedSubagent`/`parentThreadId` in
`packages/core/src/loop/workflows/agentic-execution/goal-step.ts:308-310`,
a second call site that tags forked threads the same way for a different
execution path). **On parent-thread delete, nothing cascades to fork
children or vice versa**: none of the four backends' `deleteThread`
implementations read the `parentThreadId`/`forkedSubagent` metadata keys
(confirmed by grep across `stores/` -- zero hits for either string outside
`packages/core`), so deleting a parent thread orphans its fork children
(their `metadata.parentThreadId` now points at nothing) with no detection
or cleanup mechanism. *(Inference from the absence of any cascade code, not
a directly observed failure.)*

**3. A separate, more formally-typed but seemingly-inert `harness` domain
link.** `packages/core/src/storage/domains/harness/types.ts:19-35` defines:
```ts
export interface SessionRecord {
  id: string; ownerId: string; resourceId: string; threadId: string;
  parentSessionId?: string;
  subagentDepth?: number;
  source?: { type: HarnessSessionOrigin; parentSessionId?: string; parentRunId?: string | null; parentTraceId?: string | null; subagentType?: string };
  origin: HarnessSessionOrigin;   // 'top-level' | 'subagent-tool' | 'direct-local' | 'remote-resolve'
  // ... modeId, modelId, title, metadata, state, pending, createdAt, lastActivityAt, closingAt, closeDeadlineAt, closedAt, deletedAt
}
```
and its physical schema, `HARNESS_SESSIONS_SCHEMA`
(`packages/core/src/storage/constants.ts:442-464`), has real
`parentSessionId: { type: 'text', nullable: true }` (line 447) and
`subagentDepth: { type: 'integer', nullable: true }` (line 448) columns --
this looks like exactly the durable parent-child link the other two
mechanisms lack. **However**, a grep of every non-test `.ts` file under
`packages/core/src` for `parentSessionId` finds it **only** in
`packages/core/src/storage/constants.ts:447` (the schema declaration) and
`packages/core/src/storage/domains/harness/types.ts:26,30` (the type declaration) -- no call
site anywhere in `packages/core/src` constructs a `SessionRecord` with a
populated `parentSessionId`, and `subagentDepth` likewise appears only in
those same two files. The `HarnessStorage` abstract class
(`packages/core/src/storage/domains/harness/base.ts:1-91`) exposes only
`loadSession`/`saveSession`/`listSessions` as truly abstract; `updateSession`
is a generic load-mutate-save cycle built on those three, and there is no
`deleteSession` at all -- deletion is reachable only via `updateSession(id,
{deletedAt: new Date()})`, a soft-delete-by-convention, not an enforced
schema rule (the in-memory reference implementation,
`packages/core/src/storage/domains/harness/inmemory.ts:26`, just passes
`deletedAt` through as a date-parse with no filtering of "deleted" sessions
from `listSessions()`). *(Finding, not inference: `parentSessionId` and
`subagentDepth` are real, migrated, nullable schema columns with zero
confirmed producers or consumers in `packages/core/src` at this commit -- an
apparently-aspirational or not-yet-wired field. This should be treated as
an open question, not asserted as either "used" or "dead," since it is
possible a call site exists outside `packages/core` (e.g. in a
closed-source or not-yet-explored deployment layer) that was not found by
this survey.)*

**Nesting depth**: `subagentDepth` exists in the schema (suggesting an
intended bound) but was not observed being incremented or checked anywhere
in `packages/core/src`. The `subagent` tool's *prompt text* does enforce a
depth-one limit conversationally -- a forked subagent's system prompt is
told "Do not call the `subagent` tool. You are currently running inside a
forked subagent, and this is the maximum allowed subagent nesting level"
(`packages/core/src/agent-controller/tools.ts:25`) -- i.e., recursion is
blocked by instructing the model not to recurse and by the executor
patching the tool's `execute` for forked runs, not by any storage-layer
depth field or hard guard.

## Retention, deletion, and multi-host

Retention is opt-in, table-granular, age-based, and owned by the *product*
(a configured `RetentionConfig`), never auto-scheduled by the store itself.
`RetentionConfig`/`TableRetentionPolicy`/`PruneOptions`
(`packages/core/src/storage/retention.ts:19-227`) let a caller set
`{maxAge, batchSize?}` per table key per domain (e.g.
`memory: { messages: { maxAge: '30d' }, threads: {...} }`,
`packages/core/src/storage/retention.ts:178-187`), and
`DomainRetentionTables` (`packages/core/src/storage/retention.ts:144-155`)
fixes which table keys are valid per domain -- for `memory`: `'threads' |
'messages' | 'resources'` (`packages/core/src/storage/retention.ts:145`);
for `harness`: `'sessions'`
(`packages/core/src/storage/retention.ts:153`). A call to
`MastraCompositeStore.prune(options?: PruneOptions)`
(`packages/core/src/storage/base.ts:479-502`) iterates configured domains
and delegates to each domain's own `prune()`; nothing calls this on a
timer inside the library -- the doc comments are explicit that retention
only deletes rows and never reclaims disk
(`packages/core/src/storage/retention.ts:48-51`, "On SQLite/LibSQL freed
pages are reused... Handing disk back to the OS... is left to the
underlying database and the operator"). Postgres's memory domain anchors
retention on a timezone-aware mirror column, `createdAtZ`
(`retentionTables` descriptor,
`stores/pg/src/storage/domains/memory/index.ts:161-172`), whose doc
comment states directly: "Observational memory has no timestamp anchor and
is deliberately excluded"
(`stores/pg/src/storage/domains/memory/index.ts:165-166`) -- Observational
Memory is a durable record with no age-based retention path on this
backend. Pruning is cooperative and resumable: `PruneOptions` supports
`maxBatches`, `maxRows`, `pauseMs`, and an `AbortSignal`
(`packages/core/src/storage/retention.ts:53-84`), and `PruneResult.done:
false` signals a caller should call `prune()` again rather than the loop
running unbounded (`packages/core/src/storage/retention.ts:86-104`).

**Delete cascades only within one domain's own tables**, not across the
fork/parent link (see previous section) and not into vector-store data
except where a backend explicitly scans for it -- Postgres's `deleteThread`
is the one observed backend that reaches into vector-index tables at all
(scanning `pg_tables` for `memory_messages%`,
`stores/pg/src/storage/domains/memory/index.ts:772-786`); libsql,
dynamodb, and mongodb's `deleteThread` implementations touch only their
own messages+thread tables, with no vector-store cleanup step observed in
the surveyed regions of those three files.

**Multi-host**: nothing in the surveyed anchors treats multi-host/shared-
filesystem concerns as a first-class path -- every backend here is a
network database (Postgres/libSQL-over-network/DynamoDB/MongoDB), so
"multiple processes writing to the same store" is simply "multiple clients
of the same database," governed by whatever consistency guarantees that
database provides (see the atomicity divergence above). No crash-detection,
lease, or lock-file mechanism specific to multi-host session ownership was
found in `packages/core/src/storage`.

## What this implies for our Session Store (our inference)

*(Everything in this section is our inference, not a claim about Mastra's
own stated design intent.)*

- Mastra's strongest, most load-bearing evidence for "a session store" is
  the **thread/message row-set contract** (`MemoryStorage`), and it is
  genuinely backend-independent at the *interface* level -- nine methods,
  fixed signatures, implemented identically in shape across at least six
  backends. But **semantics are not backend-independent**: atomicity,
  ordering-tiebreak consistency, and delete-cascade completeness all vary,
  sometimes by explicit, deliberate design trade-off (libsql's
  and mongodb's non-transactional `deleteThread`, both with code comments
  justifying the choice). This is direct evidence that a shared interface
  contract does not, by itself, give callers a shared consistency contract
  -- if our Session Store exposes one interface across backends, we need to
  either (a) document per-backend consistency levels explicitly rather than
  implying uniformity, or (b) push harder guarantees into the interface
  itself (e.g. require atomic multi-row writes as part of the contract,
  not as an implementation detail some backends opt out of).
- The cleanest pattern worth adopting directly: **treat the vector/search
  index as purely derived**, keyed by a stable message ID, rebuildable from
  the durable row set, updated synchronously at write time rather than via
  a background job
  (`packages/core/src/processors/memory/semantic-recall.ts:534-657`). This keeps "what is
  authoritative" unambiguous.
- The messiest pattern worth deliberately avoiding: Mastra has **three
  different parent-child linking conventions for child sessions**
  (string-concatenated ID, opaque-metadata tag, and a real-but-apparently-
  unused schema column) that do not interoperate and were evidently added
  at different times by different subsystems. A single Session Store should
  pick exactly one durable parent-child representation (a first-class
  column, not opaque metadata) and make every producer of child sessions go
  through it, specifically so a later auditor doesn't have to grep three
  places to find out whether "delete parent" orphans children.
- Mastra's working-memory/Observational-Memory/semantic-recall split shows
  that "session state beyond the raw transcript" is not one thing -- a
  design that pre-declares a single mutable "memory blob" would have missed
  this: Mastra ended up with a mutable single-field working memory, a
  versioned-generation reflection record (OM), and a derived vector index,
  each with different consistency/versioning needs. Our Session Store
  should not assume derived/secondary memory is monolithic.
- `updateMessages`' deep-merge-not-replace behavior on `content.metadata`
  (libsql, `stores/libsql/src/storage/domains/memory/index.ts:854-868`) is
  a useful precedent for how a mutable-but-still-append-log-adjacent field
  (message annotations/metadata added after the fact, e.g. approval status)
  can coexist with an otherwise-immutable message body, without treating
  the whole message row as freely rewritable.

## Open questions

- Whether `stores/*` backends not read for this dossier (clickhouse,
  cloudflare-d1, convex, dsql, mssql, mysql, redis, spanner, upstash)
  reproduce the same atomicity/ordering divergences found in pg/libsql/
  dynamodb/mongodb, or add new ones. Only the shared abstract interface and
  a few error-message strings (which name pg/libsql/mongodb/convex as
  Observational-Memory-capable, `packages/core/src/storage/domains/memory/base.ts:87`)
  were checked for these.
- Whether `HARNESS_SESSIONS_SCHEMA.parentSessionId`/`subagentDepth`
  (`packages/core/src/storage/constants.ts:447-448`) are populated anywhere
  outside `packages/core/src` -- e.g. in a deployer, a cloud/hosted control
  plane, or the `ee/` tree (which was not searched, since anything found
  there could not be cited as open-source precedent anyway). This survey
  found zero producers/consumers within `packages/core/src`, but that is
  not proof the field is dead product-wide.
- Whether the vector index has an explicit, callable "rebuild from message
  table" utility, or whether recovery from a lost/reset vector store is
  purely an operational/manual procedure. No such function was found in
  `packages/core/src/processors/memory/semantic-recall.ts`, but the file
  was not read in its entirety (roughly lines 380-660 of a larger file were
  read).
- Whether DynamoDB's emulated offset pagination genuinely costs O(N) to
  reach a deep page (re-reading from the start each time) -- this was
  reported by an earlier deep-read of
  `stores/dynamodb/src/storage/domains/memory/index.ts` but the specific
  cost claim was not re-derived line-by-line in this final verification
  pass; the pagination-helper call site
  (`stores/dynamodb/src/storage/domains/memory/index.ts:376,378`) was
  confirmed to exist, but its algorithmic cost was not re-traced.
- Whether any code path anywhere in the product (not just
  `packages/core/src`) reconstructs sub-agent hierarchy from the
  `${threadId}-${uuid}` cosmetic ID pattern used by the agent-to-agent
  delegation path (`packages/core/src/agent/agent.ts:4716-4731`) -- this
  survey's grep was scoped to `packages/core/src` and found nothing, but
  deployers/integrations/UI packages elsewhere in the monorepo were out of
  scope and not checked.
- No file-state/environment-checkpoint concept tied to conversation turns
  was found in the surveyed storage anchors; whether Mastra has such a
  concept in a different subsystem (e.g. its workflow/tool-execution layer)
  was out of scope for this dossier and was not investigated.
