# Aider compared to our session event catalog

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT_COMPARISON](../../RESEARCH_PROMPT_COMPARISON.md).
Stage-one dossier: [Aider](./index.md).
Compared against `proto/trogonai/session/sessions/v1alpha1/` and [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) on 2026-08-04.

**Store maturity: 4/12** ("thin evidence" per the research prompt's rule for
scores under 6; weight everything below accordingly): evolution scars 0/3
(no format-version field anywhere, `.aider.chat.history.md` has never needed
one because nothing reads it back by default; the parser is a heuristic
Markdown line-classifier that "will silently accept a hand-edited or foreign
Markdown file" with no version check to reject it, per the dossier's Entry
structure section); operational age 1/3 (Aider the product is old and widely
used, but the store mechanism itself shows no scarring: the dossier's own
Open questions section states plainly that "whether unbounded growth ... is a
reported real-world pain point ... could not be confirmed from source," and
no migration, no format change, and no fix-for-a-reported-failure was found
anywhere in the tree); exposure 1/3 (real, vendor-shipped, widely used
distribution, but the one behavior that would exercise the store under real
use, resume, is off by default: `restore_chat_history` defaults to `False`,
`aider/args.py:289-294`, so the store is not something the typical run
depends on at all); design independence 2/3 (the append target is a plain,
self-invented file convention, not forked from another product's persistence
code, but it is barely a design: independent because there is nothing to
copy, not because a considered alternative was rejected).

## The one structural difference everything else follows from

Aider draws the durability boundary around the **workspace**, not the
**conversation**. The only things that survive a restart are git commits
made in the working tree and a rebuildable code-search cache
(`aider/repomap.py:43`); the one artifact that looks like a session record,
`.aider.chat.history.md`, is fire-and-forget output for humans, not read back
by the program unless `--restore-chat-history` is explicitly passed
(`aider/args.py:289-294`), and its own FAQ frames that opt-in read as seeding
a *new* session with recent context, not resuming a specific prior one
(`aider/website/docs/faq.md:142`). There is no session id, no keying scheme
beyond the working directory (`aider/args.py:271-276`), and no operation that
lists, deletes, or forks a "session," because nothing is addressed as one.

Every other difference in this document is a consequence of that one choice:
no identity means no addressable resume target; no read-back means no
schema-validation pressure and no need for a format-version field; no
persisted parent/child relationship (only in-process mode objects sharing
one file, `aider/coders/base_coder.py:125-181`) means no cascade question to
answer; no store to grow means no retention question the product has ever
had to face. Our own design draws the boundary the opposite way: the
conversation, tool calls, and delegation graph are the durably owned record
([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decisions 1-6), and the workspace is a referenced, versioned fact
(`SessionStarted.workspace`, a `WorkspaceRef`) rather than the thing that
*is* durable. Aider is the cleanest evidence in the corpus that these two
durability concerns can be fully decoupled, because it is the one product
that chose to durably keep only one of them.

## Mapping

| Aider | Ours | Verdict |
| --- | --- | --- |
| `.aider.chat.history.md`, appended per UI event, write-mostly (`aider/io.py:1117-1136`, `:789`, `:795`, `:905`, `:923`, `:960`, `:970`, `:973`, `:999`) | `UserMessageRecorded`, `AssistantMessageStarted`/`Completed`, typed and always folded into the model-visible context ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 8) | Ours, decisively: theirs is not even read back on a normal run |
| Chat-started marker, `"\n# aider chat started at {current_time}\n\n"` (`aider/io.py:336`) | `SessionStarted{session_id, execution_plan, workspace}` (`proto/trogonai/session/sessions/v1alpha1/session_started.proto:16-24`) | Semantic mismatch: theirs is a human-readable timestamp string with no identifier at all; ours is a typed creation fact carrying an opaque `session_id` and the immutable plan/workspace binding |
| No session id; identity is the working directory / git root expressed as fixed file names (`aider/args.py:271-276`) | Opaque `SessionId`, one logical stream per session on `session.sessions.events.<session_id>` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 1) | Gap, deliberate on Aider's part: there is nothing to list, resume by id, or reference from outside the one shared file |
| `self.aider_commit_hashes = set()`, in-process only, gates `/undo` (`aider/coders/base_coder.py:349`) | `FileChanged{before_ref, after_ref, tool_call_id, turn_id}` folded from the durable log (`proto/trogonai/session/sessions/v1alpha1/file_changed.proto:17-43`) | Ours, decisively: Aider's record of "which commits did this session make" evaporates on process restart, which is exactly why `/undo` explicitly refuses a prior process's commits (`aider/commands.py:574`) |
| `/undo`, a git `reset`-equivalent on the working tree (`aider/commands.py:560-618`) | `SessionRewound{keep_through}` (`proto/trogonai/session/sessions/v1alpha1/session_rewound.proto:13-19`), a durable, replay-time reinterpretation of the conversation log | Semantic mismatch: "rewind" means "revert git-tracked files for commits this same process made" in Aider, and "mark events after a boundary invalid at replay" in ours; neither is a superset of the other, and ours has no effect on workspace file contents at all |
| `ChatSummary`, in-memory only (`aider/history.py:7-13`), invoked from `Coder.__init__` after a restore (`aider/coders/base_coder.py:523`) or on edit-format switch (`:158-166`) | `Compacted{summary_id, summary_content, covers_from, covers_through, trigger}` (`proto/trogonai/session/sessions/v1alpha1/compacted.proto:19-30`), a durable in-stream marker | Ours, decisively: Aider's summary is discarded every time the process exits, so a restored session that needs summarizing redoes the work from scratch |
| Mode switching (`/code`, `/ask`, `/architect`) via `Coder.create(from_coder=...)`, sharing one `io` instance and one file (`aider/coders/base_coder.py:125-181`, `:146`, `:171-179`; `aider/coders/architect_coder.py:9-46`) | `DelegationDispatched`, `ParentLinked`, `CascadePolicy` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6) | Gap, deliberate: Aider has no subagent concept at all, only an in-process object handoff with no persisted parent-child link and no independent child lifecycle |
| No retention policy, no TTL, no delete verb; `check_gitignore` only excludes the files from the user's own git history (`aider/main.py:155-171`, `:163-164`) | `SessionHidden`, `RedactionApplied`, `ArtifactErased` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7) | Ours, decisively: no analog of any kind exists in Aider |
| No format-version field; heuristic Markdown line-classification parser, silently accepts foreign input (`aider/utils.py:148-188`) | Typed protobuf events, schema-validated at the append and replay boundary (`validate_session_event`, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3) | Ours, decisively |
| `.aider.tags.cache.v{3,4}/`, a rebuildable code-search cache keyed by file identity (`aider/repomap.py:43`, `:186-224`) | Not a session concept on either side; closest conceptual analog is a rebuildable projection ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8), but keyed by conversation, not source file | Not comparable: different problem (code search vs. conversation state), noted for completeness only |
| `/save` / `/load`, file-context macros that replay a command script (`aider/commands.py:1497-1522`, `:1465-1493`) | No equivalent; this is not a session fact in our model either | Neither side models this; the naming collision is called out under **What not to copy** |
| `--restore-chat-history`, default `False`, full-file eager read with no cursor when enabled (`aider/args.py:289-294`; `aider/coders/base_coder.py:519-523`) | Runtime resumes the aggregate from the newest snapshot and replays only the tail after it ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8) | Ours, decisively: Aider's opt-in restore is O(whole file) every time it is used, with nothing bounding that cost |
| `WorkspaceRef.revision` recorded on `SessionStarted` (`proto/trogonai/session/sessions/v1alpha1/workspace.proto:16-21`) | No equivalent; Aider records nothing about the repository's state at conversation start beyond whatever HEAD happened to be | Ours: we already durably record the source-control revision a session began at, something Aider's own in-memory commit tracking never attempts |
| No lock, no fsync, no torn-write handling; concurrent writers interleave undetected (`aider/io.py:1131-1136`) | Every write path carries an explicit `WRITE_PRECONDITION` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2), enforced by JetStream at the broker | Ours, decisively |

## What we should consider changing

### 1. No change is proposed from this product's evidence

**The change.** None.

**Evidence anchor.** Aider, store maturity 4/12: the product's entire
durability investment is git commits plus a rebuildable code-search cache;
the conversation itself is, by the dossier's own framing, "negative-space
confirmation rather than a pattern to import." There is no schema, no
migration, no identity scheme, no compaction shape, no retention mechanism,
and no subagent model to compare a `.proto` field against.

**Blast radius.** None (no change proposed).

**Why.** A store scoring 4/12 is explicitly "thin evidence" under the
research prompt's own rule, and Aider is thin in the specific sense that
matters here: it is not a weaker version of the pattern we are building, it
is a deliberate decision not to build the pattern at all. The one point
Aider makes with real force, that workspace durability and conversation
durability are separable concerns, is already reflected in our design
(`SessionStarted.workspace` as a referenced fact, not the authoritative
record; `FileChanged` records facts about the workspace without owning
it) and was already raised as a recommendation from the fx comparison
(`docs/research/session-store/products/fx/vs-session-events.md`,
recommendation 8, on lifting workspace binding out of the opaque execution
plan). Citing that existing recommendation is the correct move here, not
re-deriving it from a much weaker data point.

**Cost.** None.

## What our design already does better

- **Durable attribution of file changes versus an in-memory-only commit
  set.** Aider's record of "which commits this session made" is
  `self.aider_commit_hashes`, a Python `set()` that is empty again on every
  process restart, which is why `/undo` explicitly refuses to touch a commit
  made by a prior process (`aider/coders/base_coder.py:349`,
  `aider/commands.py:573-574`). Our
  `FileChanged{before_ref, after_ref, tool_call_id, turn_id}` is durable,
  replay-stable, and answers "what did this session change and which call
  did it" indefinitely after the process that made the change is gone.
- **A durable compaction marker versus a summary that evaporates.**
  `ChatSummary` (`aider/history.py:7-13`) exists only in process memory and
  is rebuilt on the next restore; `Compacted` (`compacted.proto`) is a single
  durable fact with an explicit covered range, so a resumed session never
  has to redo work it already paid for.
- **Typed, schema-validated events versus a heuristic parser with no version
  field.** `split_chat_history_markdown` (`aider/utils.py:148-188`) infers
  message boundaries from Markdown prefixes and will silently accept a
  hand-edited or foreign file; our append and replay boundary rejects a
  malformed event before it is ever persisted ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3).
- **A guarded write path versus an unlocked, unmanaged append.** Aider opens
  `.aider.chat.history.md` with plain `open(..., "a")`, no lock, no fsync, no
  torn-write detection, and two concurrent processes can interleave lines
  undetected (`aider/io.py:1131-1136`). Every one of our writes carries an
  explicit `WRITE_PRECONDITION` enforced by the broker ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2).
- **A real retention and redaction contract versus none.** `SessionHidden`,
  `RedactionApplied`, and `ArtifactErased` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7) have no
  analog anywhere in Aider; the only "lifecycle" action found is excluding
  the files from the user's own git history via `.gitignore`
  (`aider/main.py:155-171`), which is not a retention policy at all.

## Trade-offs, not gaps

- **Zero session-store overhead versus resumability, audit, and multi-writer
  safety.** Aider's choice buys a single-user, single-host CLI zero schema
  to version, zero storage to manage, and zero migration risk, at the cost of
  no crash resume, no audit trail, and no protection against two people
  editing the same checkout at once. Our design pays a real schema and
  write-path cost to buy exactly those three things. Neither choice is wrong
  for its product; Aider simply never needed what our platform's multi-user,
  multi-host, audited use case requires.
- **One shared per-repo file versus a per-invocation opaque identity.**
  Every invocation against a given git checkout appends to the same
  `.aider.chat.history.md` (`aider/args.py:271-276`), so two people or two
  terminals share one undifferentiated transcript with no author field. This
  is a deliberate simplicity trade: it needs no setup and works with any
  existing checkout. Our opaque `SessionId` per run trades that zero-setup
  convenience for real per-run addressability and isolation.
- **Restore-as-reseed versus resume-as-replay.** Aider's own documentation
  frames `--restore-chat-history` as bringing recent context into a *new*
  session, explicitly not resuming a specific prior one
  (`aider/website/docs/faq.md:142`). That is a real, considered product
  stance, not an oversight: it is closer in spirit to our fork semantics
  (`SessionForked`, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 5, inheriting a context prefix by
  reference into a genuinely new session) than to our resume semantics
  ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8, replaying the tail of the *same* aggregate). Aider
  simply never separates the two ideas the way our design does.

## What not to copy

- **Plain append with no lock and no fsync.** `open(path, "a")` /
  `write()` / implicit close, with no temp-file-and-rename and no explicit
  fsync (`aider/io.py:1131`), and no detection of interleaved writes from
  concurrent processes. This is exactly the multi-writer hazard our
  per-command `WRITE_PRECONDITION` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2) exists to prevent.
- **Silently disabling writes on error instead of a typed failure.** A
  `PermissionError`/`OSError` on append prints a warning and sets
  `self.chat_history_file = None`, permanently disabling further writes for
  that process with no typed, observable failure (`aider/io.py:1133-1136`).
- **A parser with no format-version field that accepts anything.**
  `split_chat_history_markdown` (`aider/utils.py:148-188`) has no version
  check and no rejection path for a foreign or hand-edited file; our
  storage boundary validates every event's shape before it is trusted
  ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3).
- **In-memory-only undo-ability with no durable record of scope.** Gating
  `/undo` on a per-process `set()` that is empty after every restart
  (`aider/coders/base_coder.py:349`) means the product's only rewind
  mechanism silently narrows in scope depending on when the process last
  restarted, with no durable fact stating what "this session already
  touched" actually means.
- **Naming that does not match the mechanism.** `/save` and `/load`
  (`aider/commands.py:1497-1522`, `:1465-1493`) sound like session
  persistence but are file-context macros; neither touches
  `done_messages`/`cur_messages` or the chat history file. This is the same
  class of drift the Cline comparison flagged for "shadow Git repository"
  checkpoint documentation
  (`docs/research/session-store/products/cline/vs-session-events.md`): keep
  [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)'s own vocabulary (harness recovery checkpoint, aggregate
  snapshot, read-side checkpoint) precise for the same reason.

## The two gaps the industry has not closed

### Subagent cascade

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6 already takes a position: a child session is its own
logical stream linked by facts on each side (`DelegationDispatched` /
`ParentLinked`), acyclic by construction, with terminal cascade driven by a
reconciler and rewind-invalidation kept distinct from terminal cascade. The
question is whether Aider's evidence tests that position, not whether we
have one.

**What Aider does.** Nothing that resembles a subagent exists. Mode
switching (`/code`, `/ask`, `/architect`) is `Coder.create(from_coder=...)`
copying `done_messages`, `cur_messages`, and `aider_commit_hashes` directly
between two in-process Python objects that share the same `io` instance
(`aider/coders/base_coder.py:125-181`, `:146`, `:171-179`). Architect mode
goes one step further and builds a fresh `editor_coder`, runs it, then folds
its state back into the architect coder (`aider/coders/architect_coder.py:9-46`),
but this is an in-memory, same-process, same-file handoff: there is no
nested session directory, no parent-child link recorded anywhere durable,
and no child-transcript isolation.

**Does this validate, challenge, or refine decision 6?** Neither. Aider
plainly has no position on subagent cascade, because it has no subagent: if
the process crashes mid-"architect" turn, the entire in-memory chain,
parent and child mode object alike, disappears together, trivially and
atomically, precisely because neither one ever had independent durable life
outside that one process's memory. There is no parent to terminate, no
child to orphan, and no lineage to reconcile. This is worth recording
plainly rather than stretched into either supporting or challenging
evidence: Aider is simply not a data point on this question, and treating
its absence of a subagent model as validation of our reconciler-based
cascade would overstate what the evidence shows.

### Retention on an unbounded log

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7 already takes a position: keep-forever, with
`SessionHidden` as a visibility tombstone, `RedactionApplied` for read-time
masking, `ArtifactErased` for artifact-byte destruction, and
snapshot-bounded replay so resume cost tracks the tail, not total log size.

**What Aider does.** `.aider.chat.history.md`, `.aider.input.history`, and
`.aider.llm.history` are all open-ended append targets for the lifetime of
the repo checkout, with no explicit truncation, rotation, or size cap found
anywhere in the tree. The only lifecycle action taken on any of them is
`check_gitignore` excluding the `.aider*` glob from the user's own git
history (`aider/main.py:155-171`, `:163-164`), which keeps them out of
version control, not bounded in size. The dossier is explicit that it could
not confirm from source alone whether this unbounded growth is a
user-visible pain point in practice: no size-warning code path and no linked
issue were found in the tree either way.

**Does this validate, challenge, or refine decision 7?** It weakly
corroborates the shape of the concern decision 7 already answers, but it is
not confirmed field evidence the way, for example, the Cline comparison's
`cline/cline#9011` growth failure is
(`docs/research/session-store/products/cline/vs-session-events.md`). Two
things are worth separating here, both flagged as inference, not fact:

- Aider's default path never reads the file back at all, so unbounded
  growth on that path costs nothing at read time; the growth risk is purely
  disk usage, not a replay-cost problem, because there is no replay.
- The opt-in `--restore-chat-history` path *does* re-expose the same shape
  of risk our snapshot-bounded design is built to avoid: a full,
  eager, single-pass read of the entire file with no cursor or pagination
  (`aider/coders/base_coder.py:519-523`), so a large accumulated history
  makes every restore linearly more expensive, with nothing bounding that
  cost the way `SessionOrdinal`-anchored snapshots bound ours. But this path
  is rarely exercised (default off) and, per the dossier's own Open
  questions, has no corroborating issue report the way Cline's does.

Given the maturity score is under 6, this must be labeled thin evidence, not
presented as an industry norm: Aider suggests the same class of risk exists
whenever a store reads its history back in full, but it supplies no
confirmed failure, only an absent one that could not be ruled out either way.
It does not add new weight to decision 7 beyond what stronger stores in the
corpus already established; it is consistent with, not additional support
for, the retention design already in place.

## Open questions for the ADR

- Should [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)'s Context or Consequences state explicitly that durable,
  resumable, structured session storage is a deliberate product bet rather
  than a technical necessity for a useful coding agent, given that at least
  one widely used, long-lived product in this corpus ships without one? This
  does not change any decision; it changes how confidently the ADR can lean
  on "every serious product has converged on this shape" as a justification,
  since Aider is direct evidence that convergence is not universal.
- Is the boundary between "resume this exact session" ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8,
  replay the tail of the same aggregate) and "start a new session seeded
  with a prior transcript's tail" (closer to decision 5's fork, inheriting
  context by reference) worth surfacing as two distinct, named user-facing
  operations? Aider conflates the two under one flag and its own
  documentation is explicit that it means the latter, not the former
  (`aider/website/docs/faq.md:142`); our design already keeps them as
  separate primitives, and this is only a question of whether that
  separation should be made more visible to whatever calls the store.
