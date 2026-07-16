# Hermes Agent (Nous Research): what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Disambiguation: "Hermes Agent" =
[`NousResearch/hermes-agent`](https://github.com/NousResearch/hermes-agent),
not the Hermes open-weight model family or OpenClaw. At retrieval on
2026-07-13, the MIT-licensed repository declared package version `0.18.2`
and had 214,238 GitHub stars. It ships an OpenClaw migration command,
`hermes claw migrate`.

## Evidence anchors

The source snapshot is repository commit
[`2d71e2f1`](https://github.com/NousResearch/hermes-agent/commit/2d71e2f1e451a84239ae9d013275bf29c47ddad1),
retrieved 2026-07-13. Commit-pinned anchors keep implementation claims
auditable after `main` changes.

- Identity, license, migration command, and package version:
  [`README.md`](https://github.com/NousResearch/hermes-agent/blob/2d71e2f1e451a84239ae9d013275bf29c47ddad1/README.md#L5-L29),
  [`README.md` migration command](https://github.com/NousResearch/hermes-agent/blob/2d71e2f1e451a84239ae9d013275bf29c47ddad1/README.md#L105-L117),
  and [`pyproject.toml`](https://github.com/NousResearch/hermes-agent/blob/2d71e2f1e451a84239ae9d013275bf29c47ddad1/pyproject.toml#L8-L23).
- Product and cache design: [`AGENTS.md`, What Hermes
  Is](https://github.com/NousResearch/hermes-agent/blob/2d71e2f1e451a84239ae9d013275bf29c47ddad1/AGENTS.md#L7-L27).
- Runtime object: [`run_agent.py`,
  `AIAgent`](https://github.com/NousResearch/hermes-agent/blob/2d71e2f1e451a84239ae9d013275bf29c47ddad1/run_agent.py#L393-L489).
- Delegation contract and limits:
  [`tools/delegate_tool.py`, module contract](https://github.com/NousResearch/hermes-agent/blob/2d71e2f1e451a84239ae9d013275bf29c47ddad1/tools/delegate_tool.py#L1-L16),
  [`delegate_task` and batch limits](https://github.com/NousResearch/hermes-agent/blob/2d71e2f1e451a84239ae9d013275bf29c47ddad1/tools/delegate_tool.py#L2369-L2473),
  [background delegation lifetime](https://github.com/NousResearch/hermes-agent/blob/2d71e2f1e451a84239ae9d013275bf29c47ddad1/tools/delegate_tool.py#L3267-L3272),
  [`cli-config.yaml.example`](https://github.com/NousResearch/hermes-agent/blob/2d71e2f1e451a84239ae9d013275bf29c47ddad1/cli-config.yaml.example#L1076-L1093),
  and the official [Subagent Delegation](https://hermes-agent.nousresearch.com/docs/user-guide/features/delegation)
  guide.
- Configuration and cache economics:
  [`cli-config.yaml.example`](https://github.com/NousResearch/hermes-agent/blob/2d71e2f1e451a84239ae9d013275bf29c47ddad1/cli-config.yaml.example)
  and [`AGENTS.md`, prompt caching](https://github.com/NousResearch/hermes-agent/blob/2d71e2f1e451a84239ae9d013275bf29c47ddad1/AGENTS.md#L16-L27).
- Sessions and persistence: [`docs/session-lifecycle.md`, session
  model](https://github.com/NousResearch/hermes-agent/blob/2d71e2f1e451a84239ae9d013275bf29c47ddad1/docs/session-lifecycle.md#L56-L95),
  [gateway agent cache](https://github.com/NousResearch/hermes-agent/blob/2d71e2f1e451a84239ae9d013275bf29c47ddad1/docs/session-lifecycle.md#L568-L578),
  [`hermes_state.py`, schema](https://github.com/NousResearch/hermes-agent/blob/2d71e2f1e451a84239ae9d013275bf29c47ddad1/hermes_state.py#L141-L148),
  and the official [Sessions](https://hermes-agent.nousresearch.com/docs/user-guide/sessions)
  guide.

## The `agent` noun (primary-source quotes)

- README: "The self-improving AI agent built by Nous Research. It's the only
  agent with a built-in learning loop: it creates skills from experience,
  improves them during use, nudges itself to persist knowledge, searches its
  own past conversations, and builds a deepening model of who you are across
  sessions."
- AGENTS.md: "Hermes is a personal AI agent that runs the same agent core
  across a CLI, a messaging gateway (Telegram, Discord, Slack, and ~20 other
  platforms), a TUI, and an Electron desktop app. It learns across sessions
  (memory + skills), delegates to subagents, runs scheduled jobs, and drives
  a real terminal and browser."
- The runtime object is `class AIAgent` ("AI Agent with tool calling
  capabilities... manages the conversation flow, tool execution, and
  response handling"), constructed per session with identity params
  (`session_id`, `parent_session_id`, `platform`, `user_id`, `thread_id`,
  `gateway_session_key`) and a synchronous `run_conversation()` loop bounded
  by `max_iterations` and an iteration budget.
- **Conceptual model: agent-as-personal-daemon with a split identity.** One
  logical agent identity persists as *files* (`MEMORY.md`, `USER.md`,
  `~/.hermes/skills/`, `SOUL.md`); the process-level agent is an `AIAgent`
  instance per session, LRU-cached by the gateway (128 entries, 1h idle TTL)
  to keep prompt caches warm. Like Devin, there is one agent and many
  sessions, but self-hosted, and the "definition" is accumulated memory
  rather than platform config.

## Subagents

- Dynamic, tool-spawned: `delegate_task` "spawns child AIAgent instances
  with isolated context, restricted toolsets, and their own terminal
  sessions. Supports single-task and batch (parallel) modes... Each child
  gets: a fresh conversation (no parent history), its own task_id..., a
  restricted toolset..., a focused system prompt built from the delegated
  goal + context. **The parent's context only sees the delegation call and
  the summary result, never the child's intermediate tool calls or
  reasoning.**"
- The schema tells the model the isolation contract directly: "Be specific
  and self-contained: the subagent knows nothing about your conversation
  history."
- **Roles as capability tiers:** `leaf` (default, cannot delegate, no
  memory/clarify/send_message/execute_code/cronjob) vs `orchestrator`
  (re-granted the delegation toolset; gated by
  `delegation.orchestrator_enabled` and `max_spawn_depth`).
- **Limits are explicit config:** `max_spawn_depth: 1` ("flat by default:
  parent (0) → child (1); grandchild rejected unless raised", comment: each
  level "multiplies API cost"), `max_concurrent_children: 3` (excess
  batch tasks are rejected), child `max_iterations: 50`.
- **Lifetime coupling:** parent interrupt propagates to `_active_children`;
  synchronous children close with the parent turn; deleting a parent session
  cascade-deletes delegate children. With `background=true`, delegation can
  outlive the current turn but remains process-local. Work that must survive a
  process restart belongs in `cronjob` or `terminal(background=True,
  notify_on_complete=True)`.
- A second, cheaper delegation mechanism exists: `execute_code`, "write
  Python scripts that call tools via RPC, collapsing multi-step pipelines
  into zero-context-cost turns."
- Subagent sessions are persisted like any session (source `subagent`,
  `_delegate_from` marker) but hidden from session search/pickers.

## Configuration surface (what, where, why)

- **One YAML file:** `~/.hermes/config.yaml` (secrets in `~/.hermes/.env`).
  Major sections: `model` (provider/key/context/timeouts), `agent`
  (`max_turns` 60, reasoning effort, verify-on-stop, personalities),
  `terminal` (backend: local/ssh/docker/singularity/modal/daytona,
  sandboxing is a backend choice), `compression` (threshold 0.5, target
  ratio 0.2, protected head/tail turns), `memory` (char limits, nudge
  intervals), `delegation`, `skills` (creation nudges, external dirs),
  `session_reset` (idle/daily), `mcp_servers`, `platform_toolsets`
  (per-channel tool restriction), `tool_loop_guardrails`, `prompt_caching`,
  `auxiliary` (per-task model overrides for vision/compression/search),
  `cronjobs`, `stt`/`tts`, `profiles`, `curator` (background skill
  maintenance), `honcho` (cross-session user modeling).
- **Rationales are written into the config comments:** memory limits "keep
  the memory small and focused"; compression exists because context
  "approaches model's context limit"; session resets because "without
  resets, conversation context grows indefinitely which increases API
  costs"; delegation depth because "each level multiplies API cost."
- **The overriding design constraint is prompt-cache economics** (AGENTS.md
  design lens): "Per-conversation prompt caching is sacred... Anything that
  mutates past context, swaps toolsets, or rebuilds the system prompt
  mid-conversation invalidates that cache," and "The core is a narrow waist;
  capability lives at the edges", which is why skills inject as *user
  messages*, not system-prompt edits.

## Binding time

- **Install/config time:** provider, toolsets per platform, memory/skill
  settings, terminal backend, SOUL.md persona.
- **Session creation:** `session_id` minted (`YYYYMMDD_HHMMSS_<8hex>`);
  system prompt assembled once (MEMORY.md + USER.md + context files +
  SOUL.md); ~60 constructor params bound.
- **Mid-run:** deliberately minimal to protect the cache; model switch
  (`/model`) is the sanctioned exception; compression *splits the session*
  (new session chained via `parent_session_id`, `end_reason='compression'`)
  rather than mutating it; skills arrive as user messages.
- **Versioning:** DB schema (`SCHEMA_VERSION = 21`) and config migrations
  are versioned; skills carry a `version` in SKILL.md frontmatter; the agent
  itself has no versioned definition; identity is a living accumulation of
  memory files.

## Relationships between nouns

- **1 agent identity → N session_keys** (deterministic routing key:
  `agent:main:{platform}:{chat_type}[:{chat_id}][:{thread_id}][:{user_id}]`)
  → each key maps to **N historical sessions, one active** (resets and
  compression mint new session ids on the same lane).
- **Session (SQLite row)** → N messages; 0..N delegate child sessions
  (FK `parent_session_id` + `_delegate_from`); `end_reason` taxonomy:
  compression, branched, session_reset, agent_close, subagent,
  orphaned_compression, suspended.
- **AIAgent (process object)** 1:1 with active session; holds
  `_active_children`. **task_id** routes terminal/container isolation
  (sibling subagents share the parent's container). **thread_id** maps
  platform threads.
- State lives in `~/.hermes/state.db` (SQLite WAL + FTS5 search over own
  history), memory in markdown files, skills on disk.

## Lifecycle

- **Run:** user-run process (`hermes` CLI or `hermes gateway start`), fully
  self-hosted; no managed platform. The loop is synchronous per turn; the
  gateway dispatches per-platform messages; cron scheduler fires scheduled
  jobs; subagents run in a thread pool.
- **Pause/resume:** not first-class on the object; the gateway suspends and
  resumes *sessions* (crash recovery marks `resume_pending`; transcripts
  reload intact).
- **Stop:** `interrupt()` → loop exit → `end_session(reason)`; children
  closed with the turn.
- **Persists:** everything: full transcripts + FTS index, MEMORY.md,
  USER.md, skills, config. The learning loop (memory nudges every ~10
  turns, skill-creation nudges every ~15, a background curator that
  archives stale skills) makes persistence the product.

## What makes it "an agent" here (our inference)

Our inference: Hermes defines the agent as **a continuously-learning
personal identity hosted by a disposable process**. The durable thing is not
a config record but an accumulation, memory files, a user model, and
self-authored skills, while sessions and even the process are cheap,
cache-optimized incarnations. Its subagent design is the most
cost-explicit in the study (flat depth by default because "each level
multiplies API cost"; summaries-only back to the parent), and its deepest
design lens, prompt-cache preservation dictating what may bind mid-run,
is an operational concern no managed platform in the study surfaces as a
first-class definition constraint.

## Open questions

- The learning loop's failure modes (bad self-written skills, memory drift)
  and the curator's guarantees are not deeply documented.
- Multi-user semantics: `group_sessions_per_user` and per-user session
  lanes exist, but authorization boundaries between users of one gateway
  are unclear from the sources read.
- `honcho` (cross-session user modeling service) introduces an external
  dependency whose data model wasn't examined.
- How the Electron app and CLI share one identity concurrently (locking on
  state.db) was not covered.
