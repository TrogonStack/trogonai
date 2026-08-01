# Research Prompt: Agent Client Protocol (ACP)

Reusable prompt for the ACP study. Output goes into `docs/research/acp/`,
following the corpus structure in [index.md](./index.md): fifteen product
dossiers under `products/`, a cross-cutting [synthesis](./synthesis.md), a
[decision record](./decision-record.md), and a set of component-level deep
dives (crate inventory, tier 2 client profiles, host role and invocation,
channel mapping, bridge mechanics, file/media pipeline, permission decision
point, secrets at spawn, media store, sandboxed workspaces, ACP vs A2A).

## Driving question

This platform is a Rust agentic runtime whose client-to-agent boundary is ACP
over NATS. How does this platform use ACP to accomplish the agentic runtime,
and how can it call or host external agent CLIs (Claude Code, Gemini CLI,
Codex CLI, Goose, Grok CLI, and the rest of the product list below) through
that boundary?

Every product dossier ends with a "callability" verdict: can the platform
spawn or host this product as an ACP agent today, with what exact invocation,
auth, and blockers; and if it has no ACP support, what the nearest bridge is.

## Disambiguation (locked)

- IN scope: Agent Client Protocol (Zed, `agentclientprotocol.com`), wire v1
  stable, wire v2 draft.
- OUT of scope: IBM/BeeAI's Agent Communication Protocol (a different,
  unrelated ACP that merged into A2A in 2025), and the Agentic Commerce
  Protocol.
- Adjacent protocols (MCP, A2A, AG-UI) appear only for layer-boundary
  comparison, not as full surveys of their own.

## Research questions

### RQ1: Protocol contract (what the spec actually requires)

1. Wire framing, initialization and capability negotiation, session lifecycle
   (new, load, resume, prompt turn, out-of-turn updates, cancel semantics).
2. Permission model: `session/request_permission` flow, where authz decisions
   can hook in, what the spec leaves to the client.
3. Client-owned surfaces: filesystem RPCs, terminal RPCs, content blocks,
   tool-call presentation lifecycle.
4. v1 vs v2 draft: a detailed breaking-change diff (what v2 removes or
   breaks: the MCP HTTP+SSE transport, `session/load`, the client fs/terminal
   surface, prompt lifecycle changes), maturity signals, and adoption
   criteria. Deep enough to pre-plan an eventual migration under this
   repository's conformance policy.
5. SDK vs wire versioning: crate version vs wire version, unstable feature
   flags gating v2 types, schema crate version deltas.
6. ACP + MCP composition: MCP server config per session, shared content
   blocks, MCP-over-ACP draft RFD status.

### RQ2: Ecosystem in practice (how adopters actually leverage ACP)

The emphasis is how they use it, not just a list of who adopted it.

1. Integration patterns, as case studies: for each notable client (Zed,
   JetBrains, VS Code, Neovim, Emacs, Obsidian, Marimo, OpenClaw) and agent
   (Claude, Copilot, Codex CLI, Gemini CLI, Goose, Cline, Devin, OpenHands),
   how the integration is wired: native implementation vs. adapter/shim,
   subprocess stdio vs. remote transport, who owns the process lifecycle,
   how permissions and fs/terminal RPCs are actually surfaced in the UI.
2. Channel mapping: do products map non-editor channels (Slack, Telegram,
   WhatsApp, web chat, mobile, voice) into ACP sessions, or do they stop at
   the editor boundary? OpenClaw is the known case study (ACP as internal
   bridge protocol behind a multi-channel gateway); find who else does this,
   what patterns they use (one session per channel conversation, replay via
   `session/load`, out-of-turn `session/update`), and where ACP fights them.
   Equally important: document who deliberately does not use ACP for this
   and what they use instead.
3. Transport mapping beyond stdio: who runs ACP over WebSocket, HTTP, or a
   message bus (this platform's NATS binding as the local reference point),
   and what each had to reinvent (auth, reconnection, multi-peer routing).
4. Rust crate inventory, usable today: the official family
   (`agent-client-protocol`, `-schema`, `-http`, `-polyfill`, `-rmcp`,
   `-conductor`, `-tokio`, `-trace-viewer`) plus third-party/community
   crates. For each: maturity, maintenance health, what is already adopted
   vs. could be adopted.
5. SDKs in other languages (TypeScript, Python, Kotlin, Elixir) briefly, for
   ecosystem-health signal.
6. Governance and strategy (short): who controls the spec, velocity and
   breaking-change discipline, what Zed, JetBrains, and OpenClaw each gain,
   the governance risk of betting on a Zed-controlled protocol.
7. Positioning vs. A2A and AG-UI: which layer each owns, where they overlap.

### RQ2b: Product list (curated, drives RQ2 case studies)

Tier 1 gets a full dossier per product (one file each under `products/`).
Tier 2 gets a short profile inside the tier-2 deep dive. Anything not listed
is out of scope even if it appears in search results.

Tier 1: deep case studies (how they leverage ACP, integrations, channel
mapping, what this platform can copy or must avoid).

- **OpenClaw**: multi-channel gateway (WhatsApp, Telegram, etc.) using ACP as
  the internal bridge for hosting external coding agents. The channel-mapping
  case study.
- **Zed**: reference ACP client, spec owner. How the canonical client wires
  process lifecycle, permissions UI, fs/terminal RPCs.
- **Claude Code**: how a first-party agent gets bridged into ACP via an
  official adapter, what the adapter owns vs. the agent.
- **Gemini CLI**: launch partner, native ACP agent. The native-agent wiring
  reference.
- **Codex CLI**: adapter story for OpenAI's agent.
- **Goose**: native ACP agent from Block; also interesting because it is
  itself an extensible agent platform.
- **JetBrains**: the big-IDE adoption; how a non-Zed vendor implements the
  client side and what they pushed back on in the spec.
- **OpenCode**: does it speak ACP or deliberately not, and why? The "chose
  differently" case study.
- **Hermes Agent**: how it handles the client boundary and channels, ACP or
  not.
- **Buzz**: Block's agent platform with Nostr-based agent identity. How it
  relates to ACP and Goose (same vendor), and how its identity model
  intersects the client boundary.
- **Cursor**: does it implement or ignore ACP, and why? The dominant AI
  editor as a data point on whether the market leader sees value in the
  protocol; how it wires its own agent/client boundary and background agents
  instead.
- **Grok CLI** (xAI): ACP status, native or adapter; how it handles the
  client boundary.
- **Devin**: remote/autonomous agent with ACP support; how a cloud-hosted
  agent maps onto a protocol designed around local subprocesses.
- **NetClaw**: full case study on how it handles channels and the agent
  boundary, ACP or not.
- **Cline**: VS Code-native agent with ACP support; how an extension-first
  agent wires the client boundary, native vs. adapter.

Tier 2: short profiles (adoption shape only).

- Neovim (`avante.nvim` / `codecompanion` or equivalent ACP plugin) and Emacs
  (`agent-shell`): community client implementations, health signal.
- Obsidian: ACP client status.
- Marimo: notebook-as-client, a non-editor client shape.
- OpenHands: agent with ACP support, one paragraph on native vs. adapter.

### RQ3: Fit and roadmap (the payoff section)

Grounded only in verified repository state, not memory:

1. What this platform already has: the ACP crate family, transport adoption,
   conformance policy, relevant ADRs. Defer to
   [ACP Conformance](../../architecture/acp-conformance.md) for the exact
   version table rather than repeating numbers that will drift.
2. What ACP buys this platform: a client surface for the gateway, hosting
   external coding agents, editor integrations, session fork and
   elicitation.
3. Channel decision: should this platform map its own non-editor channels
   (web console, chat surfaces, A2A callers) into ACP sessions the way
   OpenClaw does, or keep ACP strictly as the editor/agent boundary? Answer
   grounded in the RQ2.2 findings and the existing protocol/transport
   layering ADRs.
4. Crate adoption delta: given the RQ2.4 inventory, which not-yet-adopted
   crates (polyfill, rmcp bridge, conductor) are worth adopting, and when.
5. Known gaps to weigh: any dead-code or half-wired paths recorded in the
   conformance matrix, MCP-over-ACP methods unrouted, provider ops blocked on
   upstream, no service crate wiring the ACP-over-NATS bridge to a spawn
   path yet, permission passthrough with no policy engine, secrets-in-ACP
   prohibitions.
6. Risks: v2 draft churn, NATS method-mapping maintenance per SDK bump (the
   upgrade ritual), typed-decode field-drop risk, schema version cap.
7. Recommended sequencing: what to do now, next, and watch-only.

## Method

1. Primary sources first: official docs, the spec repository, SDK source,
   release notes, official adapter repos. Secondary sources only to
   triangulate.
2. Capture each material claim with its source URL and retrieval date.
   Re-verify anything time-sensitive (draft spec mechanics, unreleased
   crates) before it is cited in code or an ADR.
3. Ground RQ3 exclusively in the repository's actual state at a named
   commit, never in memory of a prior session; note the commit alongside any
   fit-and-roadmap claim.
4. Run every non-trivial claim through an adversarial second pass before it
   lands in a dossier: could this be stated more precisely, is the cited
   source actually saying this, does a more current source contradict it.
5. Fill each dossier's section skeleton in order; do not invent sections.
   Mark anything the sources leave unanswered as an open question instead of
   guessing.
