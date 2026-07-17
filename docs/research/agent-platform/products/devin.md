# Devin (Cognition): what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence retrieved 2026-07-13 from Cognition's live documentation and blog.
The documentation is mutable, so the anchors below name the pages and sections
checked rather than implying a fixed product version.

## Evidence anchors

- Product identity and fleet framing: Cognition's
  [launch post](https://cognition.com/blog/introducing-devin) and
  [May 2026 company update](https://cognition.com/blog/series-d), plus its
  [2025 performance review](https://cognition.com/blog/devin-annual-performance-review-2025).
- Managed sessions and coordination: [Devin can now Manage
  Devins](https://cognition.com/blog/devin-can-now-manage-devins) and the
  [Advanced Capabilities](https://docs.devin.ai/work-with-devin/advanced-capabilities)
  guide.
- Multi-agent design guidance: [Don't Build
  Multi-Agents](https://cognition.com/blog/dont-build-multi-agents) and
  [Multi-Agents: What's Actually
  Working](https://cognition.com/blog/multi-agents-working).
- Session schema and lifecycle: v3 [Create
  Session](https://docs.devin.ai/api-reference/v3/sessions/post-organizations-sessions#devin-mode),
  [Get
  Session](https://docs.devin.ai/api-reference/v3/sessions/get-organizations-session),
  [Archive
  Session](https://docs.devin.ai/api-reference/v3/sessions/post-organizations-sessions-archive),
  and [API release
  notes](https://docs.devin.ai/api-reference/release-notes).
- Reusable configuration: [Creating
  Playbooks](https://docs.devin.ai/product-guides/creating-playbooks#what-are-playbooks),
  [Knowledge
  Onboarding](https://docs.devin.ai/onboard-devin/knowledge-onboarding#knowledge-101),
  and [Environment
  configuration](https://docs.devin.ai/onboard-devin/environment#how-sessions-work).
- Managed-service operations: [Secrets and Site
  Cookies](https://docs.devin.ai/product-guides/secrets), [Devin
  MCP](https://docs.devin.ai/work-with-devin/devin-mcp), and
  [Usage](https://docs.devin.ai/admin/billing/usage#sleep-and-idle-behavior),
  plus [Enterprise
  Deployment](https://docs.devin.ai/enterprise/deployment/overview).

## The `agent` noun (primary-source quotes)

- "Introducing Devin, the first AI software engineer." "Devin is a tireless,
  skilled teammate, equally ready to build alongside you or independently
  complete tasks for you to review." (cognition.com/blog/introducing-devin)
- Docs: "Devin is the AI software engineer, built to help ambitious
  engineering teams crush their backlogs"; "an autonomous AI software
  engineer that can write, run and test code."
- **The inversion:** there is no user-creatable "agent" object anywhere in
  the API. The resources are sessions, playbooks, knowledge, snapshots,
  secrets, schedules/automations, tags, repos, orgs. "Agent" never appears
  as a resource type in an endpoint path. Devin *is* the agent; users create
  **sessions**, even the session ID is prefixed with the agent's name
  (`devin-abc123def456`).
- Series D framing shows the fleet mental model: "their army of Devins
  reliably executes"; "89% of code committed by our engineers is committed
  by Devin."
- **Conceptual model: agent-as-product-identity (teammate).** One agent,
  org-wide configuration (knowledge, playbooks, machine), many concurrent
  sessions. Customization configures *the teammate*, not new agents.

## Subagents

- **"Managed Devins" (March 2026):** "Devin can break down large tasks and
  delegate them to a team of managed Devins working in parallel," "each
  running in its own isolated VM." "The coordinator session scopes the work,
  monitors progress, resolves conflicts, and compiles results." Coordinator
  powers: spin up child sessions "with specific prompts, playbooks, tags,
  and ACU limits," message them, put them to sleep or terminate them,
  monitor their ACU consumption. Mechanically: "spawn child Devins... and
  coordinate their progress through an internal MCP", described as
  "experimental, requiring careful context engineering."
- API: sessions carry `parent_session_id` and `child_session_ids`; creation
  accepts `child_playbook_id`. Archiving a parent cascades to children.
- Shared: org-wide knowledge, playbooks, snapshots, secrets. Isolated: each
  session's VM.
- **Cognition is also the industry's loudest subagent skeptic.** "Don't
  Build Multi-Agents" (Walden Yan): Principle 1, "Share context, and share
  full agent traces, not just individual messages"; Principle 2, "Actions
  carry implicit decisions, and conflicting decisions carry bad results."
  Fragmented subagent context compounds miscommunication; recommendation was
  single-threaded linear agents with context compression.
- The follow-up ("Multi-Agents: What's Actually Working") narrows the
  retraction to three patterns: generator-verifier review loops (fresh
  context *helps* review, "clean context leads to a notable improvement...
  Shared context reduces effectiveness"), "smart friend" escalation between
  frontier models, and manager-child delegation, under one constraint:
  "**writes stay single-threaded and the additional agents contribute
  intelligence rather than actions.**" Also names "context rot" as the
  reason fresh-context reviewers beat shared-context ones.

## Configuration surface (what, where, why)

Configuration attaches to the org-wide teammate, in four primitives:

- **Playbooks**, "a playbook is like a custom system prompt for a repeated
  task"; sections: Procedure ("one step per line, each line written
  imperatively"), Specifications, Advice, Forbidden Actions, Required from
  User. `!macro` shortcuts; version history with revert. Rationale: use one
  when "you or your teammates will reuse the prompt on multiple sessions" or
  you keep "repeating the same reminders to Devin."
- **Knowledge**, "a collection of tips, advice, and instructions that Devin
  can reference in all sessions." Explicit onboarding metaphor: "Just like
  onboarding a new engineer, onboarding Devin requires an initial investment
  in knowledge transfer." Each item has a `trigger_description`, "not a
  keyword search, Devin uses it as a semantic cue"; retrieval is lazy
  ("when relevant, not all at once"). Scopes: org, enterprise, repo-pinned.
  Auto-ingests `.cursorrules`, `CLAUDE.md`, `AGENTS.md`, etc.
- **Devin's Machine / Snapshots**, "Devin's Workspace resets to a saved
  machine state at the start of every session"; snapshots are "'save'
  states" with startup commands (2-minute timeout each); configured via
  wizard or declarative blueprints (classic deprecated June 2026).
- **Secrets**, org/enterprise-level, auto-retrieved "as needed in future
  sessions," or per-session inline (`session_secrets`).

Config lives in the web app, the v3 API, and the Devin MCP server ("full
access to session management, playbooks, knowledge, and scheduling").

## Binding time

- **Session creation** (`POST /v3/organizations/{org_id}/sessions`) binds:
  `prompt` (required), `playbook_id`, `child_playbook_id`, `knowledge_ids`,
  `repos`, `tags`, `attachment_urls`, `platform`, `resumable` (default
  true, "whether to preserve the session's VM state"),
  `structured_output_schema`, `max_acu_limit`, secrets, and `devin_mode`
  ("normal", "fast", "lite", "ultra", or "fusion"; preview-mode access is
  feature-gated, and "fast" is documented as ~2x faster at 4x cost).
- **Mid-session:** messages resume suspended sessions automatically; the
  Ask/Agent mode toggle switches "mid-session... with your next message";
  knowledge binds *dynamically* mid-session via trigger matching (new org
  knowledge can affect running sessions, undocumented edge).
- **Versioning:** playbooks have version history; knowledge versioning is
  undocumented; there is no agent definition to version, the "definition"
  is the org's accumulated config.

## Relationships between nouns

- **Org 1:N sessions** (the Usage guide states that there are no concurrent
  session limits); org 1:N
  playbooks, knowledge items, snapshots, secrets, automations.
- **Session** → one snapshot (machine state), 0..1 playbook, dynamic
  knowledge, tags/attachments/PRs, `acus_consumed`, and possibly a parent
  and children.
- **Statuses (v3):** `new, claimed, running, exit, error, suspended,
  resuming`; running detail: `working, waiting_for_user,
  waiting_for_approval, finished`; suspension reasons include `inactivity`,
  `usage_limit_exceeded`, `out_of_credits`...
- Adjacent read-side features reveal the split between understanding and
  execution: Devin Wiki ("automatically indexes your repositories every
  couple hours"), Devin Search, indexing/retrieval products alongside the
  session loop.

## Lifecycle

- **Create** via API, Slack @mention, web app, automations (schedule/cron,
  GitHub/Linear/Jira triggers), or MCP.
- **Sleep/wake:** "most sessions will 'sleep' instead [of ending], meaning
  that you can wake Devin up and resume the session at any point"; sleeping
  sessions auto-wake on PR retriggers; VM state preserved when
  `resumable=true`.
- **End:** archive (cascades to children), expiry; "Anything generated
  during a session that is not committed to a repository or captured in a
  snapshot disappears when the session ends."
- **Who runs the loop: Cognition, entirely.** Fully managed; even the VPC
  enterprise option is Cognition-managed infrastructure.
- **The pricing unit is agent labor:** enterprise customers consume ACUs
  based on work performed in a session, including action complexity, VM time,
  and network bandwidth. Sessions carry `acus_consumed` and a
  `max_acu_limit`; Cognition's guidance is to keep prompts and sessions short
  because both quality and usage degrade as work accumulates.
- **Persists across sessions:** knowledge, playbooks, snapshots, secrets,
  session history/insights. Performance-review framing of the persona:
  "senior-level at codebase understanding but junior at execution";
  "infinitely parallelizable and never sleeps."

## What makes it "an agent" here (our inference)

Our inference: Devin defines the agent as **the employee, not the config**,
a single product-identity teammate whose "definition" is the org's
accumulated onboarding (knowledge, playbooks, machine image) and whose unit
of work is the session (a VM-backed engagement billed in ACUs of labor).
It is the managed-service extreme of the spectrum: no agent CRUD at all,
configuration-as-onboarding, delegation modeled as a manager coordinating
copies of the same employee. Its second-order lesson for a service design is
the write-path rule distilled from production: parallel agents are safe for
intelligence (review, search, advice) and dangerous for actions, keep
writes single-threaded.

## Open questions

- Whether playbook/knowledge edits affect in-flight sessions is
  undocumented (lazy knowledge recall suggests yes for knowledge).
- The internal MCP used for manager-child coordination is not publicly
  specified.
- No public state machine for child-session lifecycle coupling beyond
  archive cascade.
