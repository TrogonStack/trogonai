# ACP and this platform: fit and roadmap

Running record of the design discussion that followed the protocol and
ecosystem study in [synthesis.md](./synthesis.md). These are directional
decisions, not final commitments. This record is frozen as decision-time
input: where a conclusion here differs from an accepted record in the
[ADR index](../../adr/index.md) or from the current spec position in
[ACP Conformance](../../architecture/acp-conformance.md), the ADR or the
conformance document is authoritative.

## What this platform already has

The exact spec position, SDK pin, schema cap, and per-method conformance
status drift too often to repeat numbers here; see
[ACP Conformance](../../architecture/acp-conformance.md) for the current
version table and matrix. At the time of this study, the relevant shape was:
wire v1, the official HTTP/WebSocket transport adopted for remote traffic, a
hand-rolled NATS leg by design
([ADR#0020](../../adr/0020-acp-sdk-1x-boundary-and-bridge-traits.md)) over a
shared JSON-RPC-over-NATS codec
([ADR#0011](../../adr/0011-jsonrpc-over-nats-binding.md)), typed decode
rather than passthrough forwarding at the bridge
([ADR#0021](../../adr/0021-typed-decode-over-passthrough-forwarding.md)),
and the ACP and A2A crate families kept in separate directory groups
([ADR#0034](../../adr/0034-rust-crate-domain-grouping.md); that grouping is
navigational, not itself a dependency-boundary rule). A known defect
recorded in the conformance matrix: the `ext/session/prompt_response`
completion path has no production waiter-registration site, so the
bridge-extension's receive half is routed and tested but the completion half
is dead code in a release build. No service crate wires the ACP-over-NATS
bridge to a process-spawning path yet.

## The gap

Every major coding agent CLI ships an ACP agent mode (`gemini --acp`,
`codex-acp`, `claude-agent-acp`, `goose acp`, `opencode acp`, `cline --acp`,
`cursor-agent agent acp`, `grok agent stdio`, `devin acp`), all wire v1 over
stdio. **This platform cannot call any of them yet.** The ACP crate family
speaks the agent role and bridges NATS to HTTP/WebSocket, but nothing in it
spawns a child process and speaks the client role against that child's
stdio. The missing component is one ACP client host: something that spawns
a CLI as a stdio child, serves fs/terminal RPCs against sandboxed
workspaces, routes `session/request_permission` through a policy decision
point instead of a blind passthrough, injects provider credentials as
environment variables at spawn, and bridges the resulting session onto the
existing NATS subjects. The current pinned SDK already ships the subprocess
helper needed for the spawn side; `buzz-acp` and Devin Desktop are working
reference designs for the host role as a whole (see
[Host Role and Invocation Mechanics](./host-role-and-invocation.md)).

## Recommended build sequencing

1. **Build the ACP client host crate** (working name `acp-host`): spawn a
   CLI as a stdio child, speak the client role, serve fs/terminal RPCs
   against sandboxed workspaces, forward `session/request_permission` into a
   policy decision point instead of a blind passthrough, inject provider
   credentials as environment variables at spawn time, and bridge sessions
   onto the existing NATS subjects. Validate against the community
   conformance tools (a CLI smoke-test harness, a trace viewer for
   debugging).
2. **Fix the `ext/session/prompt_response` dead-code defect**: the waiter
   registration that would complete the bridge extension's send half is
   currently absent from production builds, so out-of-turn responses are
   logged and dropped instead of delivered. Land this before any host work
   depends on out-of-turn responses.
3. **Bump the schema pin** per the upgrade ritual in
   [ACP Conformance](../../architecture/acp-conformance.md); this is what
   unblocks the tool-call `name` conformance gap recorded there.
4. **Wire the first three agents in this order**: Gemini CLI first (native
   agent, cleanest headless auth via an API key or Vertex ADC), then
   `codex-acp`, then `claude-agent-acp`. Goose, OpenCode, Cline, Cursor,
   Grok, Hermes, and Buzz follow as demand dictates; see the
   [product dossiers](./index.md#product-dossiers) for per-product
   invocation and auth notes.
5. **Channel adapters**: adopt the OpenClaw-validated pattern from
   [Channel Mapping](./channel-mapping.md) and
   [Channel Bridge Mechanics](./bridge-mechanics.md). Channels (console,
   chat surfaces, A2A callers) act as ACP clients in front of the gateway,
   sessions map one-per-conversation, and replay goes through
   `session/load` while still on wire v1. Keep ACP as the boundary protocol
   rather than merging channel-specific semantics into ACP payloads,
   consistent with the protocol/transport layering rules in
   [ADR#0003](../../adr/0003-ai-protocol-transport-taxonomy.md) and
   [ADR#0004](../../adr/0004-protocol-and-transport-layering.md).
6. **Watch-only, unchanged**: ACP wire v2 (adopt once upstream marks it
   preview, per the conformance policy), MCP-over-ACP routing (the three
   methods stay unrouted until runner demand appears), and the
   `-polyfill`/`-rmcp`/`-conductor` companion crates (adoption triggers are
   in the [Rust Crate Inventory](./rust-crates.md)).

## Design dossiers for the acp-host build

Four component-level designs support the `acp-host` build:

- [Sandboxed Per-Session Workspaces](./sandboxed-workspaces.md): a tiered
  design combining always-on RPC-boundary validation, an OS-sandbox layer
  around a per-session git worktree with default-deny egress, and a
  policy-triggered escalation tier for higher-risk sessions.
- [Permission Decision Point](./permission-decision-point.md): a policy
  authorizer sitting behind `session/request_permission` that combines
  verified identity, delegation chain, session key, and tool risk class into
  an issue/require-interactive/deny decision, and fails closed.
- [Credential Injection at Spawn](./secrets-at-spawn.md): resolving
  provider credentials per spawn and injecting them as environment
  variables, built on the OpenBao-backed secret custody direction in
  [ADR#0023](../../adr/0023-secret-management-and-key-custody-direction.md).
  Note that the specific guarantee that spawned agent implementations never
  receive upstream provider credentials directly, and that ACP messages
  carry no upstream credentials, authorization headers, or grant tokens, is
  recorded in the draft
  [ADR#0032](../../adr/0032-model-route-and-credential-binding.md) rather
  than in ADR#0023 itself; ADR#0023 fixes the secrets-service architecture
  those guarantees are built on, but had not reached accepted status as of
  this writing.
- [Media Store Decision](./media-store.md): S3/MinIO object storage with
  lifecycle TTLs, a `media://` resource-link scheme resolved to fresh
  presigned URLs, and an inline-base64 guardrail for small payloads.

Full supporting evidence, including the callability matrix across all
fifteen products studied, is in [synthesis.md](./synthesis.md) and the
[product dossiers](./index.md#product-dossiers).
