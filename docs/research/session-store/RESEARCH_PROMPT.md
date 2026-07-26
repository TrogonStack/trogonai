# Research Prompt: how {PRODUCT} stores and resumes sessions

Reusable prompt for the session-store study. Run once per product. Output
goes into `docs/research/session-store/products/{slug}.md`, following the
section skeleton below. Add the dossier to `index.md` when done.

## Task

Research how **{PRODUCT}** ({URLS}) persists, resumes, lists, and retires
session transcripts and session state. The goal is the operational storage
model, not marketing copy: what the durable session actually is in their
API, data model, on-disk layout, and runtime, and what is derived from it.
This feeds the platform's event-sourced Session Store design, so pay special
attention to whether the durable session is an append-only log with derived
projections or a mutable record.

## Research questions

Answer every question the sources can support. Quote primary sources; mark
gaps as gaps instead of guessing.

### 1. The storage model

- What do the docs/API/source literally say the durable session is? Capture
  exact quotes.
- What is the source of truth: an append-only event/update log, a mutable
  document, a row set, a set of files? What format (JSONL, JSON, SQL rows,
  blobs)?
- Which is derived and which is authoritative? Identify caches, summaries,
  indexes, and search state that are rebuildable projections versus the
  primary record.
- Which conceptual model fits best: session-as-transcript, session-as-log,
  session-as-document, session-as-directory, session-as-row, other?

### 2. Keying and identity

- How is a session addressed? Map the key components (project/workspace/cwd
  encoding, session id, subpath/subagent suffix) and their hierarchy.
- How are session ids minted (UUID, UUIDv7, server-assigned, client-supplied)
  and does the scheme encode ordering or location?
- Is listing scoped (per project/cwd) or global? Is there cross-project
  enumeration?
- How are relocations (moved working directory, worktree changes) or renames
  reconciled to the identity?

### 3. The store interface

Always document the store's interface, whether or not it is pluggable. The
goal is a clear operational contract of every way callers read from and write
to the durable session.

- If the product exposes a store interface/protocol (a pluggable adapter, an
  SDK type, an RPC surface), capture the **full contract verbatim**: every
  method with its exact signature, which methods are required versus optional,
  and when each is invoked. Reproduce the type as-is, not a paraphrase.
- If there is no pluggable interface (the store is an internal on-disk layout,
  a private module, or ad-hoc call sites), **reconstruct the effective
  interface** from the source and present it the same way: name each operation
  (append/write, load/read, list, summarize, delete, rename, fork, etc.), its
  inputs and outputs, its ordering and consistency guarantees, and the call
  sites that invoke it. Note that this is a reconstruction, not an exported
  type, and give a repo `path:line` for each operation.
- Either way, the reader should come away knowing the complete set of
  operations against the store and their contracts, without having to infer
  them from the append/read narrative that follows.

### 4. Write and append path

- How is a new entry/turn committed: append, insert, full rewrite,
  compare-and-swap?
- What guarantees ordering (positional line order, sequence number, server
  timestamp, monotonic id)?
- What is the durability/atomicity story (locks, temp-file-and-rename,
  transactions, torn-write healing, fsync)?
- What is the concurrency model: single-writer-per-session, multi-writer,
  optimistic concurrency with an expected position? Is there any
  expected-version precondition on append?
- What are the delivery semantics to the store (best-effort, at-least-once,
  exactly-once)? Is there client-side idempotence (dedup by entry id)?

### 5. Read and resume path

- How is a session reconstructed on resume: full ordered read, incremental
  read from a cursor, replay of a log, load of a cached view?
- Does resume read the durable store or a local cache/filesystem first?
- Is there entry-level pagination, offset, or a bound on transcript size?
- What is materialized eagerly on resume versus loaded lazily?

### 6. Listing, summaries, and search

- How are sessions enumerated for a picker/list view: index, directory scan,
  query? What does it cost at scale (quote any stated numbers)?
- Is there a metadata sidecar/summary maintained at write time (a read model)?
  What fields does it denormalize, and how is it kept consistent with the log?
- Is search a separate indexed subsystem (FTS, vector, external)? How is it
  bootstrapped, updated, and kept consistent?

### 7. Entry/message structure and versioning

- Document the actual structure of what is stored, not just whether it is
  opaque. Capture the entry/message type and its fields: any envelope or
  wrapper (timestamps, method/kind tags, ids), the payload shape, how message
  types are distinguished, and how entries link into a chain or thread
  (parent/uuid references, ordering fields). Quote the type definitions.
- Is the entry opaque to the store (persisted and returned verbatim) or does
  the store parse and interpret it? What field, if any, does the store rely on
  for identity/dedup?
- How does the format evolve across product versions (schema version fields,
  additive serde defaults, legacy-format sniffing, migrations)?
- Is there versioning of the session/store format itself, and a one-way vs
  reversible migration ratchet?

### 8. Compaction and history management

- How does the model-visible view shrink (compaction, summarization,
  truncation) while the durable record persists, if at all?
- Is compaction a store concern or an upstream concern? What artifact does it
  leave in the durable log (marker, external snapshot, in-place rewrite)?
- What resume/replay behavior crosses a compaction boundary?

### 9. Rewind, checkpoints, and fork

- Are there retroactive operations (rewind, undo, branch)? Are they expressed
  as appended markers interpreted at replay, or as destructive edits?
- Are there file-state or environment checkpoints tied to turns? How are they
  stored (full content, diff, hash, dedup) and what do they cost?
- How does fork work: shared-prefix reference, copy-plus-lineage, identity
  rewrite? What lineage metadata is recorded?

### 10. Subagents and nested sessions

- How is a subagent/child session stored: nested under the parent, a
  first-class sibling session, entries in the parent transcript?
- What is the durable parent-child link, and does the child inherit or isolate
  its transcript?
- Is nesting bounded? What happens to child sessions on parent
  delete/rewind/crash (cascade, orphan, reconcile)?

### 11. Retention, deletion, and multi-host

- Who owns retention (TTL, lifecycle policy, scheduled cleanup) and does the
  store or the product enforce it?
- How does delete behave: cascade to subkeys/summaries/search, remote-first,
  no-op for append-only backends?
- How does the store behave across hosts/processes (shared filesystem
  assumptions, crash detection, network-filesystem handling, remote
  writeback)? Is multi-host a first-class path or a workaround?

### 12. Interop with foreign session stores

- Does the product read other products' native session stores (discovery,
  import, resume)? If so, which, how bounded, and read-only or converting?
  (Skip if not applicable.)

### 13. The implicit model

- Based on all the above, what makes something "a stored session" in this
  product, and how close is the durable design to an append-only log with
  derived projections? State it in one or two sentences, in our words, clearly
  marked as our inference, and note what it implies for our event-sourced
  Session Store.

## Method

1. Primary sources first: official docs, API reference, SDK source, config
   schemas, on-disk layouts, official repos. Secondary sources only to
   triangulate. When the source is a repository, pin the exact commit and cite
   `path:line` for each claim.
2. Capture each exact quote as a checked-in source excerpt. Record its direct
   URL or repo `path:line`, document section or repository symbol, source
   version or commit when available, and retrieval date. Also record an
   archive or snapshot URL when one exists and a content digest when the source
   publishes or permits one. These evidence records must remain auditable if
   the live URL changes.
3. Record the retrieval date with `date +%F`; never guess it.
4. Stay on the operational storage model. Ignore pricing, marketing claims,
   and features unrelated to how sessions are persisted, resumed, listed, and
   retired.
5. Fill the product skeleton sections in the order the product's model makes
   natural; omit a section only when the product genuinely has no such concept,
   and say so. Put anything the sources leave unanswered under **Open
   questions**.
6. Where a conclusion here would differ from an accepted record in the
   [ADR index](../../adr/index.md), the ADR is authoritative; note the
   difference rather than overriding it.

## Output skeleton (per product file)

```markdown
# {PRODUCT}: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot retrieved YYYY-MM-DD. Version-sensitive claims were checked
against these authoritative anchors:

- {primary-source anchor: doc URL, or repo + commit + relevant paths}
- {additional anchors}

## The storage model
## Keying and identity
## The store interface   <!-- verbatim type if pluggable; reconstructed operation contract otherwise. Always present. -->
## Write and append path (ordering, durability, concurrency, delivery)
## Read and resume path
## Listing, summaries, and search
## Entry/message structure and versioning
## Compaction and history management
## Rewind, checkpoints, and fork
## Subagents and nested sessions
## Retention, deletion, and multi-host
## Interop with foreign session stores   <!-- omit if not applicable -->
## What this implies for our Session Store (our inference)
## Open questions
```
