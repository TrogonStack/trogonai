# Claude Code / Claude Agent SDK: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-07-13. Version-sensitive claims were checked
against these authoritative anchors:

- Claude Code documentation,
  [How Claude Code works](https://code.claude.com/docs/en/how-claude-code-works),
  [Glossary](https://code.claude.com/docs/en/glossary), and
  [Create custom subagents](https://code.claude.com/docs/en/sub-agents),
  specifically "Choose the subagent scope," "Write subagent files," "Spawn
  nested subagents," and "Manage subagent context."
- Claude Agent SDK documentation,
  [Overview](https://code.claude.com/docs/en/agent-sdk/overview) and
  [Subagents in the SDK](https://code.claude.com/docs/en/agent-sdk/subagents).
- Claude Code documentation,
  [Agent teams](https://code.claude.com/docs/en/agent-teams), specifically
  "Compare with subagents" and "Architecture."
- Claude Agent SDK for Python 0.2.116 at commit
  [`528265f`](https://github.com/anthropics/claude-agent-sdk-python/tree/528265fa09da954f0a0da1bf31e16db32b510138),
  which bundles Claude Code 2.1.207. The source anchors are
  [`AgentDefinition`](https://github.com/anthropics/claude-agent-sdk-python/blob/528265fa09da954f0a0da1bf31e16db32b510138/src/claude_agent_sdk/types.py#L83-L102),
  [`ClaudeAgentOptions.agents`](https://github.com/anthropics/claude-agent-sdk-python/blob/528265fa09da954f0a0da1bf31e16db32b510138/src/claude_agent_sdk/types.py#L1943-L1963),
  and the pinned
  [CLI version](https://github.com/anthropics/claude-agent-sdk-python/blob/528265fa09da954f0a0da1bf31e16db32b510138/src/claude_agent_sdk/_cli_version.py#L1-L3).
- Anthropic's [Agent SDK to Managed Agents migration](https://platform.claude.com/docs/en/managed-agents/migration),
  specifically "From the Claude Agent SDK" and "What changes," anchors the
  process, persistence, and versioning comparison.

## The `agent` noun (primary-source quotes)

The answer is layered, three distinct uses of "agent":

- **The harness + model is the agent.** "Claude Code serves as the **agentic
  harness** around Claude: it provides the tools, context management, and
  execution environment that turn a language model into a capable coding
  agent." (how-claude-code-works) Glossary: "Claude Code is the harness;
  Claude is the model inside it. The harness supplies file access, shell
  execution, permission gating, memory loading, and the loop that chains
  actions together." The "agentic loop" is "gather context, take action,
  verify results, and repeat until done."
- **Subagents are markdown-file configs.** "Subagents are Markdown files with
  YAML frontmatter." "Each subagent runs in its own context window with a
  custom system prompt, specific tool access, and independent permissions."
  (sub-agents)
- **SDK agents are per-call config objects.** The Python `AgentDefinition`
  dataclass: `description`, `prompt`, `tools`, `disallowedTools`, `model`,
  `skills`, `memory`, `mcpServers`, `initialPrompt`, `maxTurns`,
  `background`, `effort`, `permissionMode` (`types.py`). Passed via the
  `agents` dict on `query()` options; "Programmatically defined agents take
  precedence over filesystem-based agents with the same name."
- **Identity is ephemeral by default:** "Each subagent invocation creates a
  new instance with fresh context." Persistence is opt-in per definition via
  the `memory` field (`user` | `project` | `local`), which persists a
  directory across sessions, memory persists, the instance does not.
- **Conceptual model: agent-as-markdown-file (config) that instantiates as an
  ephemeral process.** The definition is a file/object; the invocation is a
  fresh context window. No server-side resource, no ID, no version field.

## Subagents

The richest subagent semantics of any product studied:

- **Five declaration scopes with priority:** managed settings (org-wide, 1)
  → `--agents` CLI JSON (session, 2) → `.claude/agents/` (project, 3) →
  `~/.claude/agents/` (user, 4) → plugin `agents/` dirs (5). Project
  discovery walks up to the repo root; plugin subfolders become scoped ids
  (`my-plugin:review:security`).
- **Frontmatter schema:** `name`, `description` (required); `tools`,
  `disallowedTools`, `model` (default `inherit`), `permissionMode`,
  `maxTurns`, `skills` (preloaded), `mcpServers`, `hooks` (scoped to the
  subagent), `memory`, `background`, `effort`, `isolation: worktree`,
  `color`, `initialPrompt`.
- **Invocation is description-driven:** "Claude uses each subagent's
  description to decide when to delegate tasks", routing is an LLM decision
  over the `description` field; explicit paths are natural language,
  @-mention, or `claude --agent name` (the main session adopts the
  definition).
- **Inheritance rules are precise.** Isolated: context window ("Subagents
  receive only this system prompt plus basic environment details... not the
  full Claude Code system prompt"), no parent conversation history or tool
  results, separate prompt cache. Inherited: all tools by default (minus a
  hard-denied set: AskUserQuestion, plan-mode tools, ScheduleWakeup), model
  (`inherit`), permission context ("bypassPermissions and acceptEdits from
  parent take precedence and cannot be overridden"), CLAUDE.md at every
  level, git status snapshot, extended-thinking config. Built-in Explore and
  Plan skip CLAUDE.md and git status "to keep research fast and inexpensive."
- **Fork is a special subagent:** "A fork is a subagent that inherits the
  entire conversation so far instead of starting fresh"; same system
  prompt/tools/model as the parent, shared prompt cache. "A fork can't spawn
  another fork."
- **Nesting:** "As of Claude Code v2.1.172, a subagent can spawn its own
  subagents... A subagent at depth five doesn't receive the Agent tool and
  can't spawn further. The limit is fixed and not configurable."
- **Foreground/background:** "As of v2.1.198, subagents run in the background
  by default. Claude runs a subagent in the foreground when it needs the
  result before continuing." `background: true` forces it.
- **Communication:** prompt in, final message out, "The only channel from
  parent to subagent is the Agent tool's prompt string"; "The parent receives
  the subagent's final message verbatim as the Agent tool result." Resumable:
  the parent gets an agent ID and can continue it via SendMessage. SDK
  messages carry `parent_tool_use_id` for attribution.
- **Contrast, agent teams** (experimental): "Multiple independent Claude
  Code sessions coordinated by a team lead, with a shared task list and
  peer-to-peer messaging. Unlike subagents, which run within a single session
  and report only to the parent, teammates each have their own context
  window and you can interact with any of them directly."

## Configuration surface (what, where, why)

- **Definition-level** (frontmatter / AgentDefinition): see schema above.
  Notable knobs absent from the programmatic type: `isolation` and `color`
  are frontmatter-only; TypeScript adds
  `criticalSystemReminder_EXPERIMENTAL`.
- **Session/harness-level** (SDK `ClaudeAgentOptions` / `Options`):
  `system_prompt` (string or `claude_code` preset), `tools`/`allowed_tools`/
  `disallowed_tools`, `mcp_servers`, `permission_mode`, `model`, `effort`,
  `max_turns`, `max_budget_usd`, `cwd`, `hooks`, `agents` dict,
  `setting_sources` (which filesystem config to load), `skills`, `plugins`,
  `resume`/`continue`/`fork_session`, `session_store`,
  `enable_file_checkpointing`.
- **Where config lives:** markdown files in scope directories; JSON via CLI
  flag; code via SDK; `settings.json` (`"agent": "code-reviewer"` default
  agent). Versioning is whatever git provides, no built-in version field.
- **Stated rationale** (sub-agents, SDK overview): context preservation
  ("keeping exploration and implementation out of your main conversation"),
  constraint enforcement ("limiting which tools a subagent can use, reducing
  the risk of unintended actions"), reuse across projects, specialization
  ("focused system prompts for specific domains"), cost control ("routing
  tasks to faster, cheaper models like Haiku"), parallelization ("independent
  subtasks finish in the time of the slowest one rather than the sum").

## Binding time

- **Live reload at file level:** "Claude Code watches `~/.claude/agents/` and
  `.claude/agents/`... detects the change within a few seconds and the next
  delegation uses the updated definition, with no restart needed."
  (Exceptions: brand-new agents directories, `--disable-slash-commands`.)
  Binding is therefore **per-invocation**, not per-session: an edited .md
  file affects the next delegation, unlike Managed Agents/OpenComputer where
  sessions pin a snapshot.
- **SDK agents bind per `query()` call** and are ephemeral, no persistence
  between calls.
- **In-flight subagents** are not restated to be affected; the new definition
  applies at next invocation.
- **No versioning primitive.** Definitions are files; history is git's
  problem. This is the sharpest contrast with the server-side products
  (Managed Agents `version` field, OpenComputer revisions).

## Relationships between nouns

- **Session** = "a conversation tied to your current directory, with its own
  independent context window." The main session *is* the agent (harness +
  model + optional named definition via `--agent`).
- **Subagent ⊂ session:** "Subagents work within a single session", they are
  context-window-scoped, not sessions of their own. Lifetime bounded by the
  task; transcript persists separately
  (`~/.claude/projects/{project}/{sessionId}/subagents/agent-{agentId}.jsonl`).
- **Agent teams:** each teammate is a full independent session, the
  many-session shape that subagents deliberately are not.
- **Skill vs subagent:** skills run inline in the caller's context (no
  isolation); subagents isolate. "Consider Skills instead when you want
  reusable prompts or workflows that run in the main conversation context."
- **Hook vs subagent:** hooks are deterministic lifecycle triggers; subagent
  delegation is model-discretionary.
- **Task tool → Agent tool:** "In version 2.1.63, the Task tool was renamed
  to Agent", the delegation primitive is literally a tool call.
- **Built-ins:** Explore, Plan, general-purpose, statusline-setup,
  claude-code-guide ship as predefined subagents.

## Lifecycle

- **Invocation lifecycle:** Agent tool call → fresh context window (agent
  prompt + task message + CLAUDE.md + git snapshot + preloaded skills) → own
  agentic loop → final text-only message returned as the tool result →
  context discarded (resumable by ID while the session lives).
- **Session lifecycle:** continue (most recent), resume (by ID), fork (copy
  history). "Sessions persist the **conversation**, not the filesystem."
  Transcripts are JSONL under `~/.claude/projects/<encoded-cwd>/`; cleanup
  after `cleanupPeriodDays` (default 30).
- **Who runs the loop:** the CLI harness or the embedded SDK runtime, "the
  SDK runs the same execution loop that powers Claude Code." Customer infra,
  customer process (the SDK's explicit contrast with Managed Agents: "Runs
  in: Your process, your infrastructure" vs "Anthropic-managed
  infrastructure"; "Session state: JSONL on your filesystem" vs
  "Anthropic-hosted event log").
- **What persists:** CLAUDE.md (re-injected every turn), session and subagent
  transcripts, auto memory (`MEMORY.md` per repo), opt-in agent memory
  directories. By default nothing persists per-subagent except its final
  message in the parent transcript.

## What makes it "an agent" here (our inference)

Our inference: Claude Code defines an agent **behaviorally, not as a
resource**, anything running the gather-act-verify loop inside the harness
is an agent, and "an agent" as a noun is just a markdown/config overlay
(prompt + tool restrictions + model + permissions) applied to that loop. The
unit of reuse is a file, the unit of execution is a context window, and
identity/versioning are delegated to the filesystem and git. It is the purest
"agent = configured loop" model in the study, and its subagent machinery
(five scopes, description-driven routing, inheritance rules, depth-5 nesting,
fork, background) is the most elaborated answer to delegation, the exact
opposite end of the spectrum from Devin's agent-as-product.

## Open questions

- Agent teams are experimental and env-gated; whether the team/teammate
  becomes a first-class noun is open.
- No versioning primitive: how organizations manage agent definition drift
  across machines (managed settings scope exists, but no version pinning).
- The [TypeScript SDK changelog through 0.3.207](https://github.com/anthropics/claude-agent-sdk-typescript/blob/79b6350e13cf24af94a8d2e696a0883fd8cc55fe/CHANGELOG.md#L45-L55)
  records workflow progress events and a phase counter in the 0.3.198 entry,
  but the workflow noun and lifecycle were not deeply documented in the
  sources read.
- `isolation: worktree` semantics beyond "temporary git worktree" (cleanup,
  merge-back) not fully specified in the docs read.
