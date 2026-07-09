# Plan: Simplify ACP-over-NATS against the existing SDK 1.2.0 API

Scope: local changes only, using APIs that already exist in the pinned
`agent-client-protocol = 1.2.0`. No version bumps, no wire-format changes, no
upstream dependency. Upstream asks (method-surface metadata from
schema-generator, public envelope types, EOF option) are tracked separately and
are explicitly out of scope here.

## Why

The ACP method surface is currently spelled out by hand in five places:
`AgentHandler`/`ClientHandler` traits, the boundary adapter, the subject enums,
the NATS dispatch matches, and `NatsClientProxy`. The SDK already ships the
pieces to remove the largest of these duplications:

- `schema::v1::ClientRequest` / `ClientNotification` implement
  `JsonRpcRequest` / `JsonRpcNotification` with the full method-name table
  (upstream `schema/enum_impls.rs`), including decode by wire method via
  `parse_message`.
- `Builder::on_receive_dispatch` accepts a typed
  `Dispatch<ClientRequest, ClientNotification>`, so one registered handler can
  claim every routed method.

After this plan, the surface lives in roughly three places (traits, one
dispatch match, subject mapping), and ADR 0020's upgrade ritual shrinks from
"trait, adapter, and subject mapping" to "trait and subject mapping".

## Item 1: Collapse the inbound boundary adapter (primary win) [DONE]

File: `rsworkspace/crates/acp-nats/src/boundary/connect_agent_boundary.rs`

Replace the 16 per-method shim functions and the
`route_request!`/`route_notification!` registration chain with a single
`on_receive_dispatch` handler over `Dispatch<ClientRequest, ClientNotification>`
containing one match that delegates to the `AgentHandler` implementation.

Behavior to preserve exactly:

- Requests run through `respond_in_task` (spawned onto the connection task,
  honoring `$/cancel_request`); the dispatch callback itself must never await
  bridge work.
- Notifications spawn and log failures via `log_notification_error`.
- The explicit no-op `CancelRequestNotification` registration stays as is (it
  is protocol-level, not part of `ClientNotification`).
- Variants the bridge does not route (`MessageMcpRequest`, future additions
  behind the wildcard arm) answer `method_not_found`, matching today's
  catch-all fall-through.
- `ExtMethodRequest` / `ExtNotification` keep their current delegation to
  `ext_method` / `ext_notification`.
- `EofSignalReader`, `BoundaryExit`, and the `connect_with`/`tokio::select!`
  shutdown logic are untouched.

Mechanical notes:

- The enum's associated `Response` type is `serde_json::Value`, so the single
  responder is `Responder<serde_json::Value>` and each request arm serializes
  its typed response, the same pattern the current `ext_method` arm already
  uses.
- The match needs a `Dispatch::Response(result, router)` arm that forwards via
  `router.respond_with_result(result)` (see the `on_receive_dispatch` rustdoc
  example) so outbound-request responses keep flowing.
- Feature-gated variants (`ForkSessionRequest` under `unstable_session_fork`,
  `MessageMcpRequest` under `unstable_mcp_over_acp`) need matching `cfg` arms;
  both features are enabled workspace-wide today.
- `ConnectionClient` (outbound direction) is already a thin typed passthrough
  and does not change.

Out of the enum's reach, unchanged: the provider methods
(`ListProvidersRequest` etc.) come from `agent-client-protocol-schema`'s
`unstable_llm_providers` feature and have no `ClientRequest` variants. The
boundary does not route them today, so nothing is lost; they remain
NATS-only.

## Item 2: Evaluate enum-based decode on the NATS leg [EVALUATED: SKIPPED]

File: `rsworkspace/crates/acp-nats-agent/src/connection.rs`
(`dispatch_global`, `dispatch_session`)

Candidate change: decode incoming payloads with
`ClientRequest::parse_message(wire_method, params)` and match once, instead of
one typed-decode closure per method arm.

Verdict after Item 4 landed: skipped, not a net reduction. The reasons:

- The per-method arms are already one-liners over shared helpers
  (`handle_jsonrpc_request`, `handle_wire_notification`,
  `handle_jsonrpc_request_with_keepalive`); enum decode would replace N
  match arms with a new helper plus N match arms, relocating the surface
  without deleting a layer.
- Three cases cannot come from the enum and would force a second style next
  to it: provider methods (absent from the SDK's `JsonRpcRequest`
  registrations), prompt (needs the JetStream keepalive variant), and cancel
  (a notification).
- The boundary collapse (Item 1) already removed the layer where enum
  dispatch pays for itself; here it would just be different plumbing.

Revisit only if upstream ships method-surface metadata (PLAN Item 6 ask 1),
which would allow generating these arms instead of hand-writing either form.

## Item 3: Documentation updates [DONE]

- ADR 0020: amend the consequences section; the adapter is no longer
  per-method, so new spec methods touch trait and subject mapping only.
- `docs/architecture/acp-conformance.md`: update the upgrade ritual to match.

## Item 4: Standardize the NATS method vocabulary (ADR 0022) [DONE]

Correction discovered during implementation: the method string is never
serialized onto the NATS wire. The content-mode codec (`jsonrpc-nats`)
carries params in the body and the id in a header; the method is derived
from the subject on receive. The dialects below were internal labels
(decode context, telemetry, in-memory messages), so this normalization is
NOT a breaking wire change and peers upgrade independently. ADR 0022
records this accurately.

The method vocabulary previously used three dialects:

| Direction | Method label (before) | Canonical ACP (now) |
| --- | --- | --- |
| Agent-bound, global | `session.new`, `providers.list` | `session/new`, `providers/list` |
| Agent-bound, session-scoped | bare `prompt`, `load`, `set_mode` | `session/prompt`, `session/load` |
| Client-bound | `session/update`, `terminal/create` | already canonical |

Extension methods used three conventions at once: `ext.{name}` (agent-bound
NATS), `ext/{name}` (client-bound NATS), and the spec's `_{name}` prefix at
the byte-stream boundary (`ConnectionClient::ext_method`). Now `_{name}`
everywhere; `ext.` survives only as a subject token.

Decision input (owner, 2026-07-09): breaking changes were pre-approved when
clearly beneficial; the implementation then established none was needed.

Done: ADR 0022 written; `GlobalAgentMethod::wire_method` /
`SessionAgentMethod::wire_method` / `ClientMethod::wire_method` return
canonical names (`session/prompt`, `providers/list`, `_name` for
extensions, `_session/prompt_response` for the prompt-response extension);
subject-token parsing untouched; Bridge senders now derive method labels
from `wire_method()` instead of hardcoded literals; test expectations
updated across the four crates.

## Item 5: Alignment notes recorded, no action now

- Trait method names (`new_session`, `session_notification`,
  `create_terminal`) mirror the removed 0.10 SDK traits rather than the
  spec's snake keys (`session_new`, `session_update`, `terminal_create`).
  Renaming is compile-time churn with no wire effect; revisit only if
  metadata-driven code generation lands and settles naming in one place.
  Recorded here so the inconsistency is known, not forgotten.

## Verification

- `cargo fmt --check`, `cargo clippy`, and tests for `acp-nats`,
  `acp-nats-agent`, `acp-nats-server`, `acp-nats-stdio`.
- Existing boundary tests (`boundary/connect_agent_boundary/`) and the live
  interoperability tests in `acp-nats-server` must pass unmodified; they are
  the proof that wire behavior did not change.
- Add regression coverage: an unrouted-but-parseable method (e.g.
  `mcp/message`) still answers `method_not_found`; cancellation during an
  in-flight `prompt` still returns the SDK's `request_cancelled` error.
- Item 4 is the exception to "tests pass unmodified": it intentionally
  changes method-label strings (internal, never serialized to the wire), so
  the affected test expectations update with it and must match `meta.json`
  names verbatim.

## Item 6: Draft the upstream-asks package

Drafting is local work and in scope; filing the issues/PRs is outward-facing
and waits for explicit approval. Produce one draft per ask, each with the
muscat evidence attached (file references, line counts deleted, conformance
rows unblocked):

1. Spec repo (`agentclientprotocol/agent-client-protocol`): schema-generator
   exports the method-surface table from `meta.json` into
   `agent-client-protocol-schema` (per-method wire name, snake token, request
   and response types, notification-ness, session-scoped flag). Unblocks
   generating the subject enums, remaining dispatch match, and
   `NatsClientProxy`; turns the conformance drift check into a compile error.
2. rust-sdk: register the `unstable_llm_providers` types in the SDK's
   `JsonRpcRequest` impls (or forward the schema feature). This is already a
   tracked blocker: the conformance matrix marks `providers/list|set|disable`
   as `unrepresentable` at the byte-stream boundary for exactly this reason.
3. rust-sdk: expose the JSON-RPC envelope types and `RequestId` publicly
   (removes the `wire.rs` mirror conversions).
4. rust-sdk: an option to treat incoming EOF as connection shutdown (removes
   `EofSignalReader`).
5. rust-sdk (optional, framing matters): a reusable trait-to-handler adapter
   as a `HandleDispatchFrom` component or cookbook recipe, aligned with
   upstream's documented "closures over traits" stance; do not propose
   reinstating the removed traits.

Store drafts under `.trogonai/upstream-asks/` until approved for filing.

## Sequencing (as executed)

Items 1, 3, and 4 shipped together in PR #478
(https://github.com/TrogonStack/trogonai/pull/478), two commits. The plan
originally split Item 4 into its own PR because it was believed to be a
breaking wire change; the correction (method labels are never serialized)
removed that reason. Item 2 was evaluated after Item 4 and skipped. Item 6
remains open, on hold; PR #478's diff is the evidence for ask 1 when it
proceeds.

## Non-goals

- Deleting the bridge-owned `AgentHandler`/`ClientHandler` traits: they become
  the single authoritative statement of the routed surface and stay.
- Any change to subject design, JetStream durability, keepalives, `wire.rs`,
  or `NatsClientProxy`.
- Filing or implementing upstream changes, and any `[patch.crates-io]`
  prototyping against the local clones. Drafting the asks is Item 6; sending
  them upstream is a separate, explicitly approved step.
- Re-fixing the February 2026 review follow-ups: verified already resolved
  (duplicate prompt waiters rejected, stdio NATS connect timeout present,
  telemetry `home_dir` code removed, backpressure and panic tests exist).
- Routing coverage changes (NES, `mcp/connect|message|disconnect`): governed
  by the conformance matrix as deliberate `not routed` / `watch-only`
  positions; revisit on runner demand, not as part of this plan.
