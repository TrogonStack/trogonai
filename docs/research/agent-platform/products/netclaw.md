# Netclaw (Petabridge): what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence from the `netclaw-dev/netclaw` source (branch `dev`, v0.24.x),
its PRDs, and netclaw.dev docs. Disambiguation: Petabridge's Akka.NET
project, not `automateyournetwork/netclaw`. Repo topics acknowledge the
lineage: `openclaw`, `hermes-agent`.

## The `agent` noun (primary-source quotes)

- **The agent is the daemon, one per deployment.** "Netclaw is a
  single-process .NET 10 application that runs as an always-on autonomous
  agent" (architecture overview); "An AI agent that runs on your hardware,
  connects to the communications tools you and your team use." The PRD
  sharpens it: "Netclaw is not just a chat assistant, it is an autonomous
  operations platform that can monitor, react, investigate, delegate work,
  and manage its own schedule." (PRD-001)
- **Identity is data, not code, and file-shaped.** "Agent identity is
  stored as data (markdown files), not code. The agent reconstructs its
  personality from files on every session start." (PRD-007) The identity
  directory: `~/.netclaw/identity/SOUL.md` (personality, tone, user's
  name), `AGENTS.md` (purpose/mission, operating rules), `TOOLING.md`
  (host capabilities), assembled by `SystemPromptAssembler`, with
  project-scoped instructions resolved from `[projectDir]/.netclaw/
  AGENTS.md`, `CLAUDE.md`, `AGENTS.md`, or `CONTEXT.md` (first match).
- **Part of the charter is compiled into the binary:** the operating-rules
  `AGENTS.md` ships as an embedded resource (read-only, placeholder
  substitution at runtime), while SOUL.md/TOOLING.md are operator-written
  disk files. Public-audience sessions get a stripped variant and no
  TOOLING/project instructions.
- **Live identity binding:** at session-actor recovery, "Always read fresh
  from disk, identity file edits take effect immediately"
  (`LlmSessionActor`). Claude Code-style live reload, bound per
  session-actor start rather than per delegation.
- **Conceptual model: agent-as-daemon** (OpenClaw/Hermes personal-daemon
  family) with file-shaped identity, one process, one persona, N
  event-sourced conversation sessions inside it.

## Subagents

- **Ephemeral child actors, spawned by tool call.** `SubAgentActor`:
  "Ephemeral actor that runs a non-interactive LLM session... executes an
  autonomous turn loop with tool calls, and returns the final text
  response as a SubAgentResult to the caller. Then stops itself. **No
  persistence, no subscribers, no streaming, no compaction.**" Spawned via
  the `spawn_agent(agent, task, context)` tool; definitions are markdown
  files at `~/.netclaw/agents/*.md` (or server feeds), declared roster,
  dynamic invocation, the Claude Code pattern.
- **Trust inheritance is mandatory and fail-closed:** "A sub-agent
  inherits the spawning session's audience. A spawn with no audience is a
  programming error: defaulting to Personal would silently grant the
  sub-agent broader trust than its parent." Inherited: audience, parent
  CWD/session directory, operating rules, tool policy ("Agent definition
  tool metadata is advisory only"). Isolated: own ephemeral history, own
  system prompt (sub-agent markdown + a hardcoded "[Subagent Execution
  Contract]": "You are a headless, non-interactive worker... Do not ask
  the user clarifying questions... Always end by emitting a final output
  for the parent session."). **SOUL.md is not inherited**, persona stays
  with the main agent; children get rules, not personality.
- **Depth 1 by static denial:** `spawn_agent` is in the sub-agent's denied
  tool set ("the only static sub-agent-specific filter denies recursive
  spawn_agent delegation"). Iteration cap 30 per sub-agent; no explicit
  fan-out cap.
- **Results-only return, supervised lifetime:** result via Akka Ask
  (`SubAgentResult` → parent), then self-stop; scope id
  `{parentSessionId}/subagent/{name}/{runId}`. Tool approvals **bubble up
  to the parent session's human** through an `IParentApprovalBridge`.

## Configuration surface (what, where, why)

- **Layered file tree under `~/.netclaw/`:** `config/netclaw.json`
  (schema-validated, `additionalProperties: false` throughout),
  `config/secrets.json` (encrypted, `ENC:` prefix),
  `config/tool-approvals.json`, `config/hard-deny-overrides.json`,
  `identity/*.md`, `skills/` (native + `.system/` CDN-synced +
  `.server-feeds/`), `agents/*.md` (subagent roster),
  `schedules/reminders/`, SQLite at `netclaw.db`.
- **Config sections:** channels (Slack/Discord/Mattermost with per-channel
  `ChannelAudiences`), `Session` (turn iteration cap 60, timeouts,
  compaction tuning: threshold, snapshot interval, kept tool results/
  messages, memory distillation intervals), `Security`
  (`DeploymentPosture` Public/Team/Personal, `ShellExecutionMode`),
  `Tools` (per-audience profiles, "Audience tool grants are monotonic:
  Public ⊆ Team ⊆ Personal", plus WebFetch allowlists, hard-deny
  patterns), `Providers`/`Models` (Main/Fallback/**Compaction** as
  separate model refs), `Memory` (recall timeout 300ms, max 3 injected),
  `SkillSync`/`ExternalSkills`/`SkillFeeds`, `SubAgents`, `Daemon`.
- **Stated rationale:** "Secure failure mode: invalid policy/config blocks
  startup" (PRD-001), fail-closed as the design center, echoed in the
  security dossier's four-layer invocation stack.

## Binding time

- **Daemon start:** full schema validation; invalid config prevents
  startup; tools statically registered.
- **Config hot-reload is validate-before-restart with drain:** "Invalid
  config changes SHALL be rejected... and SHALL keep the current daemon
  instance running. Valid changes SHALL trigger coordinated daemon
  restart: close new ingress, drain active sessions, restart, relaunch
  the sessions that were active, and resume from the last durable
  checkpoint." (PRD-001), blue/green semantics inside one binary,
  enabled by event-sourced sessions.
- **Skills hot-reload without restart** (500ms-debounced watcher);
  identity files bind at session-actor recovery ("edits take effect
  immediately" on next actor start; no mid-session re-assembly, after
  compaction only the context layers re-inject, not the system prompt).
- **Audience binds per turn** from the message's channel mapping.
- **No versioning of config or persona**, files on disk, no git
  integration, no revision concept.

## Relationships between nouns

- **Daemon 1:N sessions**; session = persistent Akka actor keyed by
  channel+thread ("Session identity is Slack thread:
  `{channelId}/{threadTs}`"), `PersistenceId = session-{entityId}`.
- **Session 1:0..N ephemeral subagents** (children in the supervision
  tree); **daemon 1:1 skill registry and reminder manager** (shared);
  session's tool view = registry filtered by the turn's audience.
- **"Everything is just input":** Slack message, webhook, timer, CLI,
  all become messages to a session actor; reminders fire as
  `SendUserMessage` indistinguishable from user input (dedup via
  `ProcessedReminderIds` rebuilt from the journal).
- **The session is our "session"; the daemon is the agent identity**, the
  same split as OpenClaw/Hermes, implemented as actors.

## Lifecycle

- **Session phases:** Recovering → Ready → Processing ⇄ Compacting →
  Passivating (aborted by an incoming message). Idle passivation (default
  30 min): final memory distillation → snapshot → stop; next message
  recreates the actor and replays.
- **Event-sourced sessions, a mini-ledger inside one daemon:** persisted
  events include `TurnRecorded`, `SessionCompacted`, `SessionTitleSet`,
  and a full tool audit trail: `ToolBatchStarted`, `ToolCallRecorded`,
  `ToolApprovalRequested`, `ToolApprovalResolved`, `ToolBatchAbandoned`.
  Snapshots at passivation and after compaction. Crash recovery = journal
  replay; "if one conversation crashes, the rest keep running."
- **Who runs the loop:** the operator's own daemon (self-hosted,
  systemd user service, loopback-bound, OS-level singleton lock).
- **Persists across restarts:** all session journals + snapshots,
  cross-session memory (SQLite), skills, config, identity files.

## What makes it "an agent" here (our inference)

Our inference: Netclaw defines the agent as **a supervised, event-sourced
resident process with a file-shaped soul**, one daemon whose identity is
markdown reconstructed at every session start, whose conversations are
independently persistent actors, and whose every tool call is journaled
and policy-gated by trust audience. Its explicit anti-chatbot criteria
(always-on, durable sessions, tools, schedules, memory, trust tiers,
self-modifiable identity files) read like a checklist of the study's
convergences implemented in one binary. Three details are operationally
notable: **tool-approval events in the session journal**
(`ToolApprovalRequested/Resolved`), **fail-closed audience inheritance for
subagents** ("defaulting to Personal would silently grant broader trust than
its parent"), and **validate-before-restart config reload with session drain
and replay**.

## Open questions

- Multiple agents per daemon: not supported today (one identity); whether
  the `agents/*.md` roster ever grows into peer personas vs staying
  headless workers.
- No versioning of identity/config despite event-sourced sessions, the
  definition plane is mutable files while the execution plane is
  immutable events.
- Fan-out cap for concurrent subagent spawns within one tool batch is
  unstated.
- Pre-1.0 (0.24.x, ~4.5 months old); details will drift.
