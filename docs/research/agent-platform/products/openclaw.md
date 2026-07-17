# OpenClaw: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence from docs.openclaw.ai and the `openclaw/openclaw` source
(v2026.6.11). History: Warelay → Clawdbot → Moltbot → OpenClaw (Jan 2026,
after Anthropic trademark complaints); created by Peter Steinberger, now
stewarded by the OpenClaw Foundation (sponsors include OpenAI, GitHub,
NVIDIA, Vercel); ~247k stars as of March 2026. MIT, TypeScript.

## The `agent` noun (primary-source quotes)

- README: "OpenClaw is a *personal AI assistant* you run on your own
  devices. It answers you on the channels you already use... The Gateway is
  just the control plane, the product is the assistant."
- The formal definition (docs/concepts/multi-agent): "**Agent**: A fully
  isolated persona consisting of workspace files, authentication profiles,
  model registry, and session store. Each agent operates independently with
  its own `agentId`."
- "Each configured agent has its own workspace, bootstrap files, and
  session store." "The embedded agent runtime is OpenClaw-owned: model
  discovery, tool wiring, prompt assembly, session management, and channel
  delivery share one integrated runtime surface." (docs/concepts/agent)
- In code, `AgentConfig` (`src/config/types.agents.ts`): `id`, `default?`,
  `name`, `description`, `workspace`, `agentDir`, `model`, `skills`,
  `identity` (name/theme/emoji/avatar), `subagents`, `sandbox`, `tools`,
  `runtime`. `DEFAULT_AGENT_ID = "main"`.
- **The persona is a directory of markdown files** (`src/agents/
  workspace.ts`): `AGENTS.md`, `SOUL.md`, `TOOLS.md`, `IDENTITY.md`,
  `USER.md`, `HEARTBEAT.md`, `BOOTSTRAP.md`, `MEMORY.md` ("durable user
  preferences and behavior guidance" per the system prompt). The workspace
  is git-initialized on creation.
- **Conceptual model: agent-as-personal-daemon persona**, a long-lived
  identity (workspace + auth + sessions) hosted by a single gateway
  process. Default deployment is one agent ("main"); multi-agent is
  multiple isolated personas in one gateway, bound to channels by routing
  rules.

## Subagents

- Two spawn mechanisms behind one `sessions_spawn` tool:
  `runtime="subagent"` (native embedded runner) and `runtime="acp"`
  (external coding agents like Codex over OpenClaw's ACP protocol).
  "Sandboxed sessions cannot spawn ACP sessions because runtime='acp' runs
  on the host."
- **Config-bounded spawning** (`subagents` block): `delegationMode`
  ("suggest" | "prefer"), `allowAgents`, `maxConcurrent` (8),
  `maxSpawnDepth` (1, "no nested spawns" by default),
  `maxChildrenPerAgent` (5), `archiveAfterMinutes` (60), per-spawn model
  override, timeouts.
- **Inheritance:** cross-agent spawns use the *target* agent's workspace
  ("For cross-agent spawns, use the target agent's workspace instead of the
  requester's"); tool allow/deny lists are inherited from the parent.
- **Agent-to-agent is off by default and allowlisted:**
  `tools.agentToAgent.enabled`, "Keep off in simple deployments and enable
  only when orchestration value outweighs complexity"; ping-pong chains
  bounded by `maxPingPongTurns`. Cross-agent memory search requires
  explicit extra collections.
- **Philosophical ceiling** (VISION.md "What We Will Not Merge"):
  "Agent-hierarchy frameworks (manager-of-managers / nested planner trees)
  as a default architecture" and "heavy orchestration layers" are
  explicitly rejected.

## Configuration surface (what, where, why)

- **One JSON5 file:** `~/.openclaw/openclaw.json` (splittable via
  `$include`; editable via CLI onboarding, web Control UI, or direct edit
  with hot reload). Sections: `agents.defaults` + `agents.list[]` +
  `agents.bindings[]` (channel/account/peer/guild/team → agentId),
  `channels.*` (with `dmPolicy`/`groupPolicy`/`allowFrom`), `session`
  (dmScope: "main" | "per-peer" | "per-channel-peer" |
  "per-account-channel-peer"; reset: daily/idle), `gateway`
  (port 18789, auth, reload mode), `cron`, `hooks`,
  `heartbeat` (default every 30m; "If nothing needs attention, reply
  HEARTBEAT_OK"; lightContext / isolatedSession flags), `sandbox`
  (mode: "off" | "non-main" | "all"; scope: "session" | "agent" | "shared";
  docker backend), `tools.agentToAgent`, `tools.elevated`,
  `models.providers` (35+ providers), `mcp.servers`.
- **Persona/behavior config is markdown in the workspace**, not JSON, the
  same file-as-config pattern as Claude Code and Hermes.
- **Stated rationale, a deliberately high config bar** (repo AGENTS.md):
  "Before adding a config option or env var, first prove existing product
  behavior, provider selection, defaults, or doctor migration cannot solve
  it. Prefer removing or consolidating config/env options."
- **Security rationale is unusually explicit:** "OpenClaw is local-first
  agent infrastructure for trusted operators; it is not designed as a
  shared multi-tenant boundary between adversarial users on one gateway";
  "Adversarial-user isolation needs separate gateways (and ideally separate
  OS users/hosts)"; "Treat inbound DMs as **untrusted input**"; "Default:
  tools run on the host for the `main` session, so the agent has full
  access when it is just you."

## Binding time

- **Hot reload is the norm:** "The Gateway watches `~/.openclaw/
  openclaw.json` and applies changes automatically." Hot-applied: channels,
  agents, tools, skills, automation, sessions. Restart-only: gateway server
  settings and plugin/manifest metadata. Reload modes: hybrid (default) /
  hot / restart / off. A rate-limited `config.patch` RPC does partial
  merges.
- **Per-message binding:** agent selection happens at message time via
  deterministic routing, "most-specific wins": exact peer > parent peer >
  peer wildcard > guild+roles > guild > team > account > channel > default
  agent.
- **Versioning:** none built-in; the workspace is git-initialized and docs
  recommend version-controlling it.

## Relationships between nouns

- **Host 1:1 Gateway; Gateway 1:N agents, channels, nodes** (devices over
  WebSocket). "One Gateway per host; it is the only place that opens a
  WhatsApp session."
- **Agent 1:1 workspace, agentDir, auth profiles, session store; 1:N
  sessions.** Session key format `agent:<agentId>:<type>:<peer>` (e.g.
  `agent:main:dm:+1234`, `agent:main:subagent:1`, `agent:codex:acp:1`).
- **Session 1:1 JSONL transcript + UUID;** DM scope decides whether all DMs
  share one "main" session per agent or split per peer/channel/account;
  groups isolate per group; cron runs get "fresh session per run"; webhooks
  "isolated per hook."
- State split across per-agent SQLite (agent-scoped state/cache), a shared
  gateway SQLite (global runtime state, plugin KV), markdown memory files,
  and JSONL transcripts.
- **ACP** is the internal bridge protocol for hosting external coding-agent
  runtimes inside the gateway's session model; OpenClaw is also both an MCP
  server and MCP client registry. A `migrate-claude` extension imports
  Claude Code/Desktop config, and `hermes claw migrate` exists on the
  competitor's side, this niche has migration paths in both directions.

## Lifecycle

- **The gateway is an OS daemon** (launchd / systemd / Windows Task
  Scheduler; `openclaw onboard --install-daemon`), a long-lived Node
  process; OpenClaw's embedded runtime owns the loop, prompt assembly, and
  tool wiring.
- **Sessions** persist until reset (daily at a configured hour by default,
  idle-based, or manual `/new`); transcripts persist as JSONL regardless.
- **Heartbeat** wakes the agent every ~30m with a strict prompt contract
  (HEARTBEAT.md, "reply HEARTBEAT_OK" when idle), proactive scheduled
  cognition as a first-class lifecycle feature.
- **Persists:** workspace markdown (persona + memory), transcripts, SQLite
  state, credentials. Everything is on the operator's disk, "All state may
  contain secrets."

## What makes it "an agent" here (our inference)

Our inference: OpenClaw's agent is a **resident persona**, a named
directory of markdown (soul, identity, memory, tools) plus credentials and
a session store, animated by one always-on gateway daemon and reachable
wherever the operator already chats. Identity is file-shaped and
git-versionable, sessions are routing-scoped conversation lanes rather than
task runs, and delegation is deliberately shallow (depth-1, allowlisted,
with hierarchy frameworks explicitly refused). Together with Hermes it
defines the personal-daemon corner of the spectrum, and its sharpest
contribution to a service design is the trust-boundary statement: one
gateway equals one trusted operator; adversarial isolation belongs at the
process/host boundary, not inside the agent model.

## Open questions

- Multi-agent-per-gateway plus `agentToAgent` blurs the single-operator
  trust model; the docs punt adversarial isolation to separate gateways.
- Subagent session archival (`archiveAfterMinutes`) semantics vs transcript
  retention were not fully traced.
- ACP protocol versioning/compat guarantees for external runtimes (Codex
  harness pinned at a specific version) are unclear.
- The Foundation governance model (post-Steinberger-to-OpenAI) and its
  effect on roadmap is outside the code but relevant to adoption bets.
