# Multi-Channel Agent Routing — Q&A Session

Design conversation about routing multiple chat channels (Telegram, Discord, ...)
into a shared generic agent loop over ACP/NATS.

Context established before this session:

- One shared agent process behind `acp-nats` is the only place with a model loop.
- Channels are entry points; per-platform crates do platform I/O only.
- Reference systems studied: OpenClaw and Hermes (both single-process monoliths;
  both converge on: channel adapter -> routing policy -> session -> agent loop ->
  stream events rendered per platform).
- Model so far: endpoint -> principal -> conversation -> agent -> session.
  - Binding = endpoint references a conversation (many-to-one).
  - Routing policy consulted only at conversation creation (sticky agent).
  - Conversations are cross-channel: same conversation reachable from multiple
    endpoints (identity linking required; reply-to-origin per prompt).
  - Session (AcpSessionId) is ephemeral and belongs to the agent; conversation
    holds agentId (sticky) + currentSessionId (replaceable).

---

## 1. Document type and audience

**Question:** What kind of document is the end product, and who is it for? (a) architecture/design doc for future contributors, (b) ADR capturing decisions and rationale, (c) implementation RFC/plan to execute against, or a combination?
**Intention:** Document type dictates structure, depth, and placement; avoid writing a hybrid that serves no audience well.
**Assumptions:**
- The subject matter is the endpoint -> principal -> conversation -> agent -> session design.
- This will be committed project documentation, not a throwaway internal note.

**Answer:** "decide whatever the heck we suppose to do to bind telegram to an AI agent" — the document is a decision-oriented design doc: its job is to conclude, concretely, how Telegram gets bound to an AI agent. Not a taxonomy exercise.

## 2. The concrete agent on the other end

**Question:** What is the concrete agent for v1 — a specific ACP agent already running behind acp-nats-server, or is choosing/building the shared generic agent part of what we're deciding?
**Intention:** "Bind Telegram to an AI agent" needs a real agent at the end of the pipe; if none exists, the doc must decide that too and v1 scope changes.
**Assumptions:**
- The ACP-over-NATS infrastructure is operational and the intended transport.
- Channels must not know which agent implementation they talk to.

**Answer:** "stdio, http, ACP, A2A, sources like the telegram, multiple ins and outs" — there is no single agent endpoint. The agent side is plural and protocol-diverse: ACP agents (stdio-bridged or NATS-native), A2A agents, HTTP agents. Ingress is plural too (Telegram is one of many sources). The system is a many-ins to many-outs router; the conversation model must bind to an agent through a protocol-neutral adapter, not assume ACP/AcpSessionId specifically.

## 3. First end-to-end slice

**Question:** Is the v1 pair "Telegram in -> ACP agent out" with A2A/HTTP deferred, or is A2A the strategic priority given a2a-gateway is under active development?
**Intention:** The first protocol silently shapes the conversation/session abstraction; the doc should name the v1 pair and sketch the adapter interface against at least two protocols so the first doesn't get baked in as "the" model.
**Assumptions:**
- Telegram is the v1 ingress.
- acp-nats is closest to ready on the agent side.
- The agent adapter owns opaque session state so ACP/A2A/HTTP all fit.

**Answer:** Redirect: focus the architectural document itself. Scope it to NATS subject design and worker design — make the architecture concrete in terms of subjects and workers. (V1 protocol-pair selection not answered; architecture must stand on its own in NATS terms.)

## 4. Channel-neutral namespace vs per-channel namespaces

**Question:** Should the doc define a channel-neutral subject namespace (chat.in / chat.out with the router between) replacing tgbot.*, or keep per-channel namespaces bridged by the router?
**Intention:** Biggest subject-design decision; channel-neutral means the router is written once and new channels are edge-only work.
**Assumptions:**
- Raw ingress stays as-is: gateway owns telegram.> verbatim payloads.
- The prefix/env concept survives into the new namespace.
- Stated lean: channel-neutral; tgbot.* becomes transitional.

**Answer:** "right, so how we go from telegram.> into the agent picking up things?" — agreement with the channel-neutral lean, plus a request to spell out the concrete pipeline from raw telegram.> to agent pickup, in subjects and workers.

Pipeline proposed and shown in conversation:
telegram.{update_type} (stream TELEGRAM, raw; trogon-gateway, exists)
-> chat-edge-telegram normalize half
-> chat.{prefix}.in.telegram.{account}.{peer} (stream CHAT_IN, neutral inbound)
-> chat-router (identity, binding, conversation KV, per-convo serialization, agent dispatch)
-> agent protocol (acp-nats today)
-> router translates notifications
-> chat.{prefix}.out.telegram.{account}.{peer} (stream CHAT_OUT, render commands)
-> chat-edge-telegram render half -> Telegram API.

## 5. Agent-side: adapter workers vs library adapters

**Question:** Do agents get a protocol-neutral NATS namespace with adapter workers (agents.{prefix}.{agentId}.prompt), or does the router embed protocol adapters as libraries (speaks acp-nats client in-process; A2A/HTTP clients later)?
**Intention:** Decides whether "agent" is NATS-addressable or a router implementation detail; last piece of the worker/subject diagram.
**Assumptions:**
- acp-nats already gives NATS addressability for ACP agents; a neutral agents.> would partially duplicate it.
- "Multiple ins and outs" might imply agents as first-class NATS citizens.
- Conversation KV schema is identical either way.

**Answer:** User unsure, delegated the call. Decision (recommended and adopted): library adapters inside the router, no agents.> namespace. Rationale: protocol-neutral agent addressability already exists twice (acp-nats for ACP, a2a-gateway for A2A); a generic namespace adds a second hop and forces redesigning streaming-RPC-over-NATS, which acp-nats already solved. Neutral contract lives in code: router trait AgentPort (create/resume session, prompt -> event stream, cancel); v1 ships the ACP impl; agent registry lives in router config (agentId -> protocol + address). Revisit only if a non-router service needs to prompt agents.

## 6. chat.in / chat.out payload scope (OPEN — asked, not yet answered)

**Question:** V1 payloads: inbound { endpoint, sender, text, timestamp, message_ref }, text-only with reserved attachments field; outbound render vocabulary of four commands (send_text, edit_text, typing, react/ack). Text-only acceptable, or is media a day-one requirement?
**Intention:** Wire schemas are the hardest thing to change later; every edge and the router depend on them.
**Assumptions:**
- telegram-types' Telegram-shaped vocabulary is intentionally NOT the neutral model.
- Streaming to Telegram uses edit-in-place, hence edit_text.
- Media later goes claim-check (gateway already has ClaimCheckPublisher), not inline.

**Answer:** (pending — user responded with question 7 instead; resolved by entry 8)

## 7. Is the outbound edge an HTTP outbound gateway?

**Question (from user):** "would output bound be essentially a http outbound gateway?"
**Intention (user's, inferred):** Testing whether the render half of chat-edge-telegram is the mirror of trogon-gateway, i.e. a generic NATS -> HTTP egress component.
**Assumptions:** n/a (user-originated question)

**Answer (mine, adopted):** Transport-wise yes: it is the mirror of trogon-gateway (sources = HTTP-in -> NATS; this = NATS -> HTTP-out, a "sink"). But outbound has two jobs and only one is generic egress: (1) rendering, i.e. chat semantics (4096-char chunking, edit-in-place throttling, markdown flavor, edit-target-expired fallback) which is platform-driver logic; (2) egress execution (HTTP call, token custody, retry, rate limit). V1 keeps both fused in chat-edge-telegram. Doc names the evolution path: a generic "sink" concept in trogon-gateway symmetric to sources; strongest driver is token custody (gateway already holds TelegramBotToken for webhook registration, so the token currently lives in two places). That refactor touches neither router nor subjects, so it must not hold up v1.

## 8. Attachments in v1: eager claim-check

**Question:** Text-only v1 with reserved attachments field, or media from day one? (continuation of entry 6)
**Intention:** Lock the wire schemas; they are the contract every edge and the router build against.
**Assumptions:**
- Neutral schema not modeled on telegram-types' Telegram-shaped events.
- edit_text streaming preview wanted.

**Answer:** "attachments can come! we request or wait for such attachment downloads or however they suppose to work" — attachments are in v1, mechanism delegated. Decision (recommended and adopted): eager claim-check. At normalize time the edge downloads the media from Telegram (only the token holder can redeem a file_id), stores bytes in the object store (claim-check pattern the gateway already uses), and chat.in carries a reference { kind, mime, size, object_ref, platform_ref }. Router maps references into the agent protocol's content model (ACP content blocks / resource links). Outbound mirrors it: send_attachment render command carries object_ref; edge uploads to Telegram. Size capped by existing media_max_mb config. Lazy fetch-on-demand rejected: needs a request/reply service on the edge, fails mid-conversation, and Telegram download URLs expire.
**Supersedes:** 6

## 9. Identity storage and v1 posture

**Question:** Operator-centric v1 (principals declared statically in router config, allowlist-gated) or multi-user from day one (auto-provisioned principals)?
**Intention:** Sets access-control default, decides if pairing is v1, determines whether the principal registry is config (static) or KV (dynamic).
**Assumptions:**
- Existing AccessConfig suggests allowlist-gated posture.
- Config-declared linking suffices for a small known set of people.
- Stated lean: operator-centric v1.

**Answer:** "it would be nats jetstream or kv for things" — identity and related state live in NATS JetStream/KV, not config files. All stateful registries are KV buckets from day one: principals, endpoint->principal links, endpoint->conversation bindings, conversation records (agentId + current session). Router config stays minimal (wiring, agent registry). This makes the registry dynamic-capable regardless of whether v1 policy is allowlist-gated; the access posture question folds into how KV gets seeded/administered.

## 10. Administering the KV registries

**Question:** How do principals get into KV: bootstrap via env/config seed + in-band chat commands (/allow, /link), out-of-band tooling only (CLI mutating buckets), or both?
**Intention:** Empty bucket = nobody can talk to the bot; needs a first-principal story and an ongoing-admin story; decides whether the router grows a command vocabulary in v1.
**Assumptions:**
- Env-seeded admin is an acceptable bootstrap escape hatch.
- Chat commands are the natural admin surface (OpenClaw/Hermes do pairing in-band).
- Command vocabulary would be router-owned and channel-neutral.

**Answer:** "CLI, config files, eventually GUI, but we can keep it simple for now" — administration is out-of-band: a CLI and config-file seeding for v1, GUI later. No in-band admin commands in v1 (user-facing conversation commands like /new remain in scope; admin operations like allowing users or linking endpoints happen via CLI/config, not chat). Follow-up: "or even MCP to approve who knows, but focus on getting things to work first" — the admin surface is deliberately open-ended (CLI, config, GUI, maybe MCP); the doc should treat KV as the source of truth with the admin surface pluggable and unspecified, and not spend design effort there. Priority is the working pipeline.

## 11. Document placement

**Question:** Write the doc at docs/architecture/multi-channel-agent-routing.md, and should key decisions also be extracted into ADRs under docs/adr/?
**Intention:** Match repo documentation conventions before writing; ADR extraction only worth it if the ADR index is the real decision log.
**Assumptions:**
- docs/architecture/ is the right home for an explanation-style architecture doc.
- The .qa.md file remains the raw decision trail.
- Low-ceremony default: single doc now.

**Answer:** No ADRs: "keep that for platform level decision more than local domain reasons." Single architecture doc at docs/architecture/multi-channel-agent-routing.md.

## 12. V1 goes direct (telegram.> -> bridge -> ACP)

**Question (from user, over several exchanges):** "why from telegram, to chat, to ACP?", then "we adding latency and translations all over the place, what is the point?", then "could we go directly to the agent somehow? how does openclaw or hermes does it?"
**Intention (user's):** Challenge the three-hop layering; check whether the reference systems justify a middle namespace.
**Assumptions:** n/a (user-originated challenge)

**Answer (adopted):** Both OpenClaw and Hermes go direct because their agent is a library: the channel handler calls the agent loop as a function and consumes its stream events in-process. Our equivalent of that function call is acp-nats itself (prompt/notification traffic already crosses NATS). Decision: v1 fuses edge and router into one worker, chat-bridge-telegram, consuming telegram.> (stream TELEGRAM) directly and speaking the acp-nats client; NO chat.in/chat.out subjects in v1. Latency argument was honest but secondary (hops cost ~10-20ms vs multi-second agent turns); the real point was that the middle namespace only pays off at channel two or a second consumer. Guard against monolith rot: all channel-neutral logic (KV schemas, binding logic, AgentPort trait, inbound event + render command types) lives in a shared crate the Telegram binary imports; the chat.> namespace is reframed in the doc as the documented extraction path along those seams.
**Supersedes:** 4 (partially: chat.> becomes end state, not v1)

## 13. Telegram structure over ACP: the _meta convention

**Question (from user):** "would the ACP lose some structure or data from telegram?" followed by "ACP has the `_meta` thingy, I think for do whatever we want"
**Intention (user's):** Verify no meaningful data loss going direct to ACP; point out ACP's extensibility field.
**Assumptions:** n/a (user-originated)

**Answer (adopted):** Verified in agent-client-protocol-schema 0.10.6: _meta exists on 91 types including PromptRequest, every ContentBlock, and session notifications, reserved for arbitrary metadata; acp-nats already uses it for prompt_id correlation. Mapping: (1) content (text/media) -> ACP content blocks, no loss; (2) conversational context (sender, reply-to, group vs DM) -> dual-carried: human-readable prefix in the text block (any agent; only prompt text reaches the model) plus structured PromptRequest._meta (participating agents); _meta is machine-visible not model-visible; (3) interactivity: inbound button presses become synthetic prompt text at the bridge; outbound buttons ride notification _meta ({ telegram: { buttons } }) rendered by the bridge, ExtNotification for event-shaped extensions; non-participating agents degrade to plain text. Raw TELEGRAM stream retains full fidelity for replay regardless.
