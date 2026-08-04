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

## Wave 1 — mature stores that speak to both open gaps

| Product | Repo @ pinned commit | License | Why first |
| --- | --- | --- | --- |
| OpenHands | `OpenHands/software-agent-sdk` @ `973c35134f0b` (primary), `OpenHands/OpenHands` @ `866512a485c8` (app) | MIT | Only candidate that addresses both gaps: agent delegation recorded as events in the parent stream, and a condenser as a live retention story on an append-only log |
| Pi | `earendil-works/pi` @ `a96fb984d8c8` | MIT | Three numbered session-format versions with auto-migration on load, a checked-in format spec, and a pluggable repository interface. Young, but the strongest evolution evidence on the list |
| Cline | `cline/cline` @ `5ec2d47b21b3` | Apache-2.0 | Subtask parent/child model, plus the only documented user-visible failure of unbounded transcript growth. Failure evidence outranks another success story |
| Zed | `zed-industries/zed` @ `4aad57fd1f00` | Per-crate Apache-2.0 + GPL | Oldest codebase on the list, `sqlez` domain migrations with an explicit backfill-key pattern, and the only store whose schema is ACP-shaped. Cross-reference the ACP corpus |
| Continue | `continuedev/continue` @ `5522c6f44ca0` | Apache-2.0 | Ships legacy-format filtering in the read path, which is direct evolution evidence, and its per-session-file plus `sessions.json` index is a worked index-drift failure mode |

## Wave 2 — mature stores, narrower lesson

| Product | Repo @ pinned commit | License | Why |
| --- | --- | --- | --- |
| Amazon Q CLI | `aws/amazon-q-developer-cli` @ `15cc8f3cd18c` | Apache-2.0 | Eight named SQL migrations under a vendor-shipped CLI. Store is a single mutable `ConversationState` blob keyed by cwd, which is the degenerate endpoint of the retention spectrum: no history to retain |
| Crush | `charmbracelet/crush` @ `fcfad839bbef` | FSL-1.1-MIT | Seven goose migrations in fifteen months. `parent_session_id` plus cascade foreign keys expresses the subagent cascade policy directly in DDL. Also stores versioned file content inside the session database |
| Letta | `letta-ai/letta` @ `ff19ffeafeb5` | Apache-2.0 | MemGPT lineage, longest-running attempt at separating agent state from the message log, and an archival tier that is a real retention answer rather than an absence of one |
| Aider | `Aider-AI/aider` @ `5dc9490bb35f` | Apache-2.0 | Mature product, deliberately thin store. Expect a low maturity score despite the product's age; the finding is what a widely used tool chose *not* to persist |

## Wave 3 — younger stores and framework abstractions

| Product | Repo @ pinned commit | License | Why |
| --- | --- | --- | --- |
| Google ADK | `google/adk-python` @ `cbedafd9e4c1` | Apache-2.0 | `BaseSessionService` over in-memory, database, sqlite, and Vertex, with its own migration and schema directories. Product-side counterpart to LangGraph's checkpointer |
| OpenAI Agents SDK | `openai/openai-agents-python` @ `7b7587425a17` | MIT | A second OpenAI session model incompatible with Codex rollout files. The divergence inside one vendor is the finding |
| AWS Strands | `strands-agents/harness-sdk` @ `23541039fa1f` | Apache-2.0 | Interface-first `session/` module with file and S3 repositories. Note the repo redirect from `sdk-python`; code lives under `strands-py/src/strands/session/` |
| Mastra | `mastra-ai/mastra` @ `9e1dad8f7b1c` | Apache-2.0 with `ee/` carve-out | Thread and message shape held constant across seven backends. Evidence about which parts of a session model are backend-independent |

## Wave 4 — thin stores, short entries

| Product | Repo @ pinned commit | License | Why |
| --- | --- | --- | --- |
| SWE-agent | `SWE-agent/SWE-agent` @ `3ea751c087f3` | MIT | `.traj` trajectory files. Benchmark-driven rather than resume-driven, which makes it a clean contrast case for what a session is *for* |
| Void | `voideditor/void` @ `b3166e7ef2ae` | Apache-2.0 | Persistence layer not yet located; last commit 2026-06-02. Timebox it, and if there is no coherent store, record that as the finding and stop |

## Wave 5 — forks, delta-only

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

- `jentic/standard-agent` — its entire persistence surface is
  `agents/memory/dict_memory.py`, an in-memory dict. No durable session, so
  nothing to compare.
- Closed-source harnesses (Cursor, Amp, Copilot CLI, Windsurf, Factory
  Droid) — no primary source, and the corpus rule is primary sources first.
