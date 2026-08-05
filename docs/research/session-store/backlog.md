# Session store research backlog

Eighteen products queued for the two-stage study:
[stage one](./RESEARCH_PROMPT.md) produces the dossier,
[stage two](./RESEARCH_PROMPT_COMPARISON.md) produces the comparison against
our catalog and the ranked change recommendations.

Commits pinned 2026-08-04 against each repository's default branch. Re-pin
before starting a product if its dossier has not been written yet; these
anchors exist so a dossier can cite an exact tree, not so the backlog can go
stale quietly.

## Ordering

Two factors, in this order:

1. **Store maturity.** Evidence from a store that has migrated its own data
   under shipped users outweighs evidence from a store that has never had to.
   The rubric is in the [stage-two prompt](./RESEARCH_PROMPT_COMPARISON.md).
2. **Relevance to the two open gaps** the synthesis leaves for us: subagent
   cascade semantics and retention on an unbounded log.

Star counts deliberately do not appear below. They measure product adoption,
not whether the storage format survived contact with reality. Amazon Q CLI
has the fewest stars on this list and one of the most-migrated schemas.

## Wave 1 -- mature stores that speak to both open gaps

| Product | Repo @ pinned commit | License | Why first |
| --- | --- | --- | --- |
| OpenHands | `OpenHands/software-agent-sdk` @ `973c35134f0b` (primary), `OpenHands/OpenHands` @ `866512a485c8` (app) | MIT | Only candidate that addresses both gaps: agent delegation recorded as events in the parent stream, and a condenser as a live retention story on an append-only log |
| Pi | `earendil-works/pi` @ `a96fb984d8c8` | MIT | Three numbered session-format versions with auto-migration on load, a checked-in format spec, and a pluggable repository interface. Young, but the strongest evolution evidence on the list |
| Cline | `cline/cline` @ `5ec2d47b21b3` | Apache-2.0 | Subtask parent/child model, plus the only documented user-visible failure of unbounded transcript growth. Failure evidence outranks another success story |
| Zed | `zed-industries/zed` @ `4aad57fd1f00` | Per-crate Apache-2.0 + GPL | Oldest codebase on the list, `sqlez` domain migrations with an explicit backfill-key pattern, and the only store whose schema is ACP-shaped. Cross-reference the ACP corpus |
| Continue | `continuedev/continue` @ `5522c6f44ca0` | Apache-2.0 | Ships legacy-format filtering in the read path, which is direct evolution evidence, and its per-session-file plus `sessions.json` index is a worked index-drift failure mode |

## Wave 2 -- mature stores, narrower lesson

| Product | Repo @ pinned commit | License | Why |
| --- | --- | --- | --- |
| Amazon Q CLI | `aws/amazon-q-developer-cli` @ `15cc8f3cd18c` | Apache-2.0 | Eight named SQL migrations under a vendor-shipped CLI. Store is a single mutable `ConversationState` blob keyed by cwd, which is the degenerate endpoint of the retention spectrum: no history to retain |
| Crush | `charmbracelet/crush` @ `fcfad839bbef` | FSL-1.1-MIT | Seven goose migrations in fifteen months. `parent_session_id` plus cascade foreign keys expresses the subagent cascade policy directly in DDL. Also stores versioned file content inside the session database |
| Letta | `letta-ai/letta` @ `ff19ffeafeb5` | Apache-2.0 | MemGPT lineage, longest-running attempt at separating agent state from the message log, and an archival tier that is a real retention answer rather than an absence of one |
| Aider | `Aider-AI/aider` @ `5dc9490bb35f` | Apache-2.0 | Mature product, deliberately thin store. Expect a low maturity score despite the product's age; the finding is what a widely used tool chose *not* to persist |

## Wave 3 -- younger stores and framework abstractions

| Product | Repo @ pinned commit | License | Why |
| --- | --- | --- | --- |
| Google ADK | `google/adk-python` @ `cbedafd9e4c1` | Apache-2.0 | `BaseSessionService` over in-memory, database, sqlite, and Vertex, with its own migration and schema directories. Product-side counterpart to LangGraph's checkpointer |
| OpenAI Agents SDK | `openai/openai-agents-python` @ `7b7587425a17` | MIT | A second OpenAI session model incompatible with Codex rollout files. The divergence inside one vendor is the finding |
| AWS Strands | `strands-agents/harness-sdk` @ `23541039fa1f` | Apache-2.0 | Interface-first `session/` module with file and S3 repositories. Note the repo redirect from `sdk-python`; code lives under `strands-py/src/strands/session/` |
| Mastra | `mastra-ai/mastra` @ `9e1dad8f7b1c` | Apache-2.0 with `ee/` carve-out | Thread and message shape held constant across seven backends. Evidence about which parts of a session model are backend-independent |

## Wave 4 -- thin stores, short entries

| Product | Repo @ pinned commit | License | Why |
| --- | --- | --- | --- |
| SWE-agent | `SWE-agent/SWE-agent` @ `3ea751c087f3` | MIT | `.traj` trajectory files. Benchmark-driven rather than resume-driven, which makes it a clean contrast case for what a session is *for* |
| Void | `voideditor/void` @ `b3166e7ef2ae` | Apache-2.0 | Persistence layer not yet located; last commit 2026-06-02. Timebox it, and if there is no coherent store, record that as the finding and stop |

## Wave 5 -- forks, delta-only

Do not write full dossiers. Answer one question: what diverged from
upstream's store, with paths. "Nothing diverged" is a complete and useful
answer.

| Product | Repo @ pinned commit | License | Upstream |
| --- | --- | --- | --- |
| Roo Code | `RooCodeInc/Roo-Code` @ `b867ec914575` | Apache-2.0 | Cline |
| Kilo Code | `Kilo-Org/kilocode` @ `6ec20f23952b` | MIT | Cline, via Roo Code |
| Qwen Code | `QwenLM/qwen-code` @ `06cc41ee3f50` | Apache-2.0 | Gemini CLI |

## Where the authoritative spec lives

Located and confirmed to exist. A dossier author starts here rather than
rediscovering it, and any product whose row says *needs discovery* should be
timeboxed before it consumes a full research slot.

| Product | Authoritative types and format |
| --- | --- |
| Pi | `packages/coding-agent/docs/session-format.md` is a checked-in written spec, with version 1 linear, version 2 `id`/`parentId` tree, version 3 role rename, auto-migrated on load. Types in `packages/coding-agent/src/core/session-manager.ts` and `messages.ts`, `packages/ai/src/types.ts`, `packages/agent/src/types.ts`. Store interface in `packages/agent/src/harness/session/repository.ts` with `jsonl-repo.ts` and `memory-repo.ts` implementations, plus `scripts/migrate-sessions.sh`. The spec's links point at `pi-mono`, which is the pre-rename name of the same repo |
| OpenHands | `openhands-sdk/openhands/sdk/conversation/event_store.py`, `state.py`, `persistence_const.py`; server side in `openhands-agent-server/openhands/agent_server/persistence/{models,store}.py`. Dedicated tests at `tests/sdk/conversation/test_event_store.py`, `test_state_serialization.py`, `tests/sdk/event/test_event_serialization.py` |
| Cline | `apps/vscode/src/core/storage/disk.ts`, `StateManager.ts`, `state-migrations.ts`, with a migration test suite in `__tests__/state-migrations.test.ts`. Monorepo layout: storage moved under `apps/vscode/` |
| Zed | `crates/agent_ui/src/thread_metadata_store.rs`, `agent::ThreadStore`, and the ACP schema at `agent_client_protocol::schema::v1` |
| Crush | `internal/db/migrations/*.sql` is the schema of record; sqlc output in `internal/db/models.go`, `sessions.sql.go`, `messages.sql.go`. The `messages.parts` column is JSON, so the entry type itself lives in Go outside `internal/db` and needs discovery |
| Continue | `core/util/history.ts` and `core/util/paths.ts`; `Session` and `BaseSessionMetadata` types in `core/index.d.ts` |
| Amazon Q CLI | `crates/chat-cli/src/database/mod.rs` plus `crates/chat-cli/src/database/sqlite_migrations/*.sql`; `ConversationState` under `crates/chat-cli/src/cli/` |
| Letta | `letta/schemas/message.py`, `conversation.py`, `letta_message.py`, `agent.py`, `archive.py`; service layer in `letta/services/message_manager.py` |
| Google ADK | `src/google/adk/sessions/session.py`, `base_session_service.py`, `database_session_service.py`, plus `schemas/` and `migration/` |
| AWS Strands | `strands-py/src/strands/session/file_session_manager.py` and `__init__.py` |
| OpenAI Agents SDK | `src/agents/memory/sqlite_session.py`, reference at `docs/ref/memory.md` |
| Mastra | Storage domain interfaces under `packages/core/src/storage/`, per-backend adapters under `stores/*/src/storage/domains/memory/` |
| Aider | `aider/io.py` (`chat_history_file`). Expect no schema; the absence is the finding |
| SWE-agent | Trajectory writer in `sweagent/agent/agents.py`; entry type needs discovery |
| Void | Needs discovery. Timebox and drop if nothing coherent exists |
| Roo Code, Kilo Code, Qwen Code | Diff against upstream's paths above |

## Completed

**Wave 1, dossiers written and verified**: Cline, Continue, OpenHands, Pi, Zed.

Verification was mechanical then manual, because the two catch different
errors. Mechanically, every `path:line` in each dossier's prose was resolved
against the pinned tree: 434 citations across the five, with no unresolvable
path in any of them. Manually, each dossier's load-bearing claims were then
read back against source, because a line number that resolves says nothing
about whether the line supports the claim attached to it, and that is the
failure mode a citation checker cannot see.

That manual pass changed three things, which is the argument for doing it:

- Zed's `sqlez` ratchet was described as content-hashed. It stores each
  migration's full text and compares it after `sqlformat` normalization, with
  an escape-hatch callback. Corrected, because "hashed" would have implied a
  cheaper drift check than the one that exists.
- Zed's subagent cascade was described as breadth-first. The code pops from
  the back of a `frontier` vector, so it is depth-first. Corrected. The
  traversal order does not matter for delete-everything semantics, but the
  claim should still be true.
- Cline's cascade delete was described as a guarantee against orphaning. It is
  guarded by `if (!row.isSubagent)` and does not recurse, so it holds only
  because the graph is in practice one level deep. Added, because the
  distinction between a cascade that is transitive and one that merely looks
  transitive on a flat graph is exactly what wave 1 was commissioned to find.

One prompt defect surfaced and was fixed before wave 2 could inherit it:
dossiers cited bare basenames (`db.rs:671`), which cannot be mechanically
resolved in trees holding three `db.rs` and three `migrations.rs`, or ten
`types.ts`. The stage-one prompt now requires repo-root-relative citations.

**Waves 2 through 4, dossiers written and verified**: Aider, Amazon Q
Developer CLI, AWS Strands, Crush, Google ADK, Letta, OpenAI Agents SDK,
SWE-agent, Void.

**Wave 5, fork deltas written and verified**: Roo Code, Kilo Code, Qwen Code.

Both verification layers ran on all of these. The mechanical layer found and
fixed real citation defects rather than merely passing: Amazon Q shipped 18
unresolvable citations across two different files both named `mod.rs` in a tree
holding 43 of them, plus four more split between two files named
`checkpoint.rs`, all now fully qualified; Letta cited an Alembic revision under
an elided filename and gave a line range 105 lines past the end of
`summarizer.py`. The bare-basename defect therefore survived the stage-one
prompt fix in agents working from long context, which is worth knowing: the
rule needs enforcement, not just statement.

The manual layer again found what the mechanical layer structurally cannot,
this time in the Kilo Code delta, and the corrections mattered more than wave
1's:

- Kilo was credited with authoring a recursive cascade delete. The recursion
  carries no `// kilocode_change` marker, so it arrived with the vendored
  OpenCode core. Re-attributed, because crediting it to Kilo would have
  double-counted one upstream's evidence as two independent data points.
- The same delta asserted Roo Code has "no cascade-delete subsystem of its
  own". Roo recurses over `childIds` at
  `src/core/webview/ClineProvider.ts:1747-1762`, which the Roo delta had
  already established as the corpus's cleanest counter-example on cascade
  semantics. Corrected, and the two deltas now agree.
- The delta recommended cross-checking Kilo's engine against an OpenCode
  dossier "if this corpus later adds" one. The corpus already had one.
  Corrected to point at it, with the caveat that the two are pinned at
  different upstream generations, so a diff between them may be version skew
  rather than a Kilo patch.
- One flagged open question was resolved rather than carried: `SessionV2` is
  neither a parallel engine nor an in-progress rewrite, it is a dependency of
  the engine the report treats as authoritative.

A second prompt defect surfaced during the stage-two calibration batch (Cline,
OpenHands, Zed) and was fixed before the remaining comparisons were
commissioned. The comparison skeleton titled a section "The two open gaps",
which reads as though subagent cascade and retention are unresolved in *our*
design. They are unresolved in the industry; [ADR#0035](../../adr/0035-session-store-decider-aggregate.md) decisions 6 and 7 take
detailed positions on both. All three calibration documents got the substance
right and tested the decisions properly, so the defect was latent rather than
realized, but the heading would have propagated to twelve more documents and
misled anyone skimming. The section is now "The two gaps the industry has not
closed", and the prompt states outright that writing it as though we have no
position is the single most likely way to get a comparison wrong.

**Stage-two comparisons written and verified**: Aider, Amazon Q Developer CLI,
AWS Strands Agents, Continue, Crush, Google ADK, Letta, Mastra, OpenAI Agents
SDK, Pi, SWE-agent, Void, on top of the fx reference implementation and the
Cline, OpenHands, and Zed calibration batch. The prompt fix above held: all
sixteen use the corrected heading and test decisions 6 and 7 rather than
presenting our design as having a hole.

Mastra scores 11/12, the corpus's joint-highest store maturity alongside Letta,
and it is the first product whose evidence is a set of backends disagreeing with
each other: four adapters reach four different atomicity conclusions from one
shared abstract interface, from a real Postgres transaction down to DynamoDB's
sequential writes plus a rollback that can itself fail and is logged and
swallowed. That disagreement is the finding, not a defect in any one adapter.

Both layers ran on every one. Three defect classes recurred often enough to be
worth naming, and the third is new to stage two:

- The bare-basename defect survived a second prompt statement. Amazon Q's
  comparison reproduced the exact defect already fixed in its own dossier, two
  bare `mod.rs` citations in a tree holding 148 candidates, and Pi's comparison
  shipped four bare `types.ts` and one bare `messages.ts` in a tree holding ten
  and two respectively. Stating the rule twice is not enforcement; the checker
  is.
- Right line number, wrong file. Aider's comparison attributed
  `self.aider_commit_hashes = set()` to `aider/commands.py:349`, which is an
  unrelated `/commit` dirty check; the assignment is at
  `aider/coders/base_coder.py:349`. Pi's comparison attributed a tree-entry
  envelope to `packages/coding-agent/src/core/types.ts:375-380`, a path that
  does not exist: `SessionTreeEntryBase` is at
  `packages/agent/src/harness/types.ts:375-380` and the CLI's own
  `SessionEntryBase` is in `session-manager.ts`. Both resolve mechanically only
  if the checker is not told which tree to look in, and neither is visible
  without opening the file.
- An absence established against a file that does not exist. Pi's comparison
  supported a recommendation with "zero matches for `retainedTail` in
  `packages/coding-agent/src/core/compaction.ts`", but `compaction.ts` is a
  directory there. The finding survived on stronger evidence once re-derived,
  zero matches anywhere under `packages/coding-agent/src` against five in that
  package's own `docs/session-format.md`, which is the sharper claim. A grep
  that returns nothing because the path is wrong looks exactly like a grep that
  returns nothing because the code is absent.

Two smaller patterns were corrected across the wave rather than per-document. A
comparison citing its own dossier by line number, 39 sites in Void and 9 in
Amazon Q, now cites by section link instead, and only for grep-established
absences and dossier conclusions; the Void conversion went further and traced 27
of its 39 back to product source. And several proto citations named a field that
sat just outside the range they cited, which is why the rule is now to open the
file and confirm the field is inside the range.

**Per-artifact verification was not enough.** Running the checker once over
every artifact with a local clone found defects in six files that individual
waves had already reported clean: seven flattened paths in the Cline comparison
(each omitting a `stores/`, `services/`, or `models/` segment), ten bare
`types.ts` and `messages.ts` citations in the Pi dossier, thirteen bare
`manager.py` and `registry.py` citations in the OpenHands dossier, and, in both
Zed artifacts, a set of bare `db.rs` citations spanning two different crates,
one of which attributed `open_fallback_db` to `crates/agent/src/db.rs:215` when
it lives at `crates/db/src/db.rs:215`. The corpus now stands at 1912 citations
with everything resolved. The lesson is procedural: a per-artifact check run at
landing time uses whatever root set was convenient then, and a bare basename
unique in one root becomes ambiguous the moment a sibling root is added. Only
the whole-corpus run with the correct root set per artifact is meaningful.

Reading `open_fallback_db` in full to fix its path also closed two of the Zed
dossier's own open questions, which is worth noting as a side effect of
verification rather than a separate task: the fallback's trigger conditions are
now confirmed rather than inferred from its name, and both of Zed's identifier
mint sites turn out to be `Uuid::new_v4()`, so the identifier scheme the dossier
listed as undetermined is settled. Fixing a citation means opening the file, and
opening the file answers questions the original pass had left open.

## Stage three -- the per-provider payload catalog

Both stages so far take a product as the unit of study. Neither takes a
provider, and the omission is measurable rather than arguable. Across the whole
corpus, "Anthropic" appears four times, one of which is our own proto comment
offering `"anthropic"` as an example value; "Messages API", "Bedrock", and
"LiteLLM" appear zero times; "Responses API" appears once. There is no catalog
anywhere of Anthropic content-block types, OpenAI Responses API item types,
Google GenAI `types.Content` and `Part` variants, or Bedrock Converse blocks.

This matters because `ProviderBlock`
(`proto/trogonai/session/sessions/v1alpha1/message.proto:62-70`) exists to
absorb precisely what the typed `ContentBlock` arms cannot model. It carries a
`provider` string and a `block_type` described as "the provider's own
discriminator for the block, verbatim". We designed the escape hatch and never
enumerated what goes through it, so we cannot currently say whether the seven
typed arms are the right seven, which is the question the schema's shape
actually turns on. The fx comparison raises the same doubt from the other side
and leaves it open: whether a provider-native escape hatch belongs in a
canonical catalog at all.

Two data points bound the problem. Google ADK, the corpus's joint
second-strongest store at 10/12, inherits its payload from `LlmResponse` rather
than declaring it, so the shape a reader needs is one class away from the shape
the dossier documents. The OpenAI Agents SDK, at 5/12, stores
`TResponseInputItem = ResponseInputItemParam`, a bare alias onto the provider's
own wire type with no envelope and no version field, which means its durable
payload *is* the provider format. That is the exact failure mode `ProviderBlock`
is meant to avoid, and it is currently the corpus's only worked example of it,
supplied by one of its weakest stores.

What a stage-three prompt must answer, per provider rather than per product:
enumerate the block and item discriminators from the published API surface, mark
which map onto a typed `ContentBlock` arm and which can only land in
`ProviderBlock`, and decide whether `block_type` needs a registry or stays
genuinely opaque.

**Two dossier repairs this surfaced**, both the same defect and now covered by
rule 7 of the stage-one prompt:

- **Google ADK dossier**: document the inherited payload. `Event` extends
  `LlmResponse`, which is named twice and only as a superclass; the actual
  content field and its `parts` structure are absent.
- **Grok Build dossier**: open `SessionUpdateEnvelope.params` and
  `ConversationItem`. Both are named in the directory-contents table and never
  again, so the payload inside the source-of-truth append log is undocumented.
  This dossier also has no entry-structure section at all, unlike the other
  twenty-four.

## Verification state at queue time

The pinned commits and licenses above are verified. The store descriptions
are not uniformly verified, and the dossier author should treat them as
leads, not findings:

- **Source read this session**: Crush (full initial migration and migration
  list), Continue (`core/util/history.ts`), Zed
  (`crates/agent_ui/src/thread_metadata_store.rs`), Amazon Q CLI
  (`crates/chat-cli/src/database/mod.rs`).
- **Checked-in format spec read, source not yet read**: Pi
  (`packages/coding-agent/docs/session-format.md`).
- **Module or path listing only**: OpenHands, Cline, Google ADK, Strands,
  Letta, OpenAI Agents SDK, Mastra, Aider, SWE-agent.
- **Unverified**: Roo Code, Kilo Code, Qwen Code, Void.

Two pins moved after locating the specs. OpenHands' persistence is in
`software-agent-sdk`, not the `OpenHands/OpenHands` app repo, and Pi moved
from wave three to wave one: the maturity rubric weights evolution scars
highest, and three numbered format versions with an auto-migrating loader is
the strongest such evidence on the list, which outweighs the store's age.

## License flags

Three products need their provenance stated before anything from them is
cited as an open-source precedent:

- **Crush** is FSL-1.1-MIT, source-available rather than OSI open source,
  converting to MIT on a delay.
- **Zed** is licensed per-crate across Apache-2.0 and GPL. Check the crate a
  quote comes from.
- **Mastra** is Apache-2.0 with an `ee/` enterprise carve-out.

## Excluded, with reason

- `jentic/standard-agent`: its entire persistence surface is
  `agents/memory/dict_memory.py`, an in-memory dict. No durable session, so
  nothing to compare.
- Closed-source harnesses (Cursor, Amp, Copilot CLI, Windsurf, Factory
  Droid): no primary source, and the corpus rule is primary sources first.
