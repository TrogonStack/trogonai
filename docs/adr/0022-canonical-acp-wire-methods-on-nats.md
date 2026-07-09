---
number: "0022"
slug: canonical-acp-wire-methods-on-nats
status: accepted
date: 2026-07-09
---

# ADR 0022: Canonical ACP Method Vocabulary in the NATS Layer

## Context

The JSON-RPC method vocabulary used by the NATS layer historically had three
dialects, depending on direction and scope:

| Direction | Method label (before) | Canonical ACP |
| --- | --- | --- |
| Agent-bound, global | `session.new`, `providers.list` | `session/new`, `providers/list` |
| Agent-bound, session-scoped | bare `prompt`, `load`, `set_mode` | `session/prompt`, `session/load` |
| Client-bound | `session/update`, `terminal/create` | already canonical |

Extension methods used three conventions at once: `ext.{name}` (agent-bound),
`ext/{name}` (client-bound), and the spec's `_{name}` prefix at the
byte-stream boundary.

Importantly, these strings are not wire bytes. The content-mode codec
(`jsonrpc-nats`) never serializes the method: requests and notifications
carry only params in the body and an id header, and the receiving side
derives the method from the NATS subject. The dialects lived in
`wire_method()` labels used for decode context, telemetry, and in-memory
messages. They still carried real costs: the vocabulary did not match the
spec repository's `meta.json`, the conformance check needed a translation
table, telemetry spoke a private dialect, and decoding via the SDK's
`ClientRequest::parse_message` (which matches canonical names) was
impossible.

## Decision

The method vocabulary is the canonical ACP wire name everywhere:
`session/prompt`, `session/new`, `providers/list`, exactly as `meta.json`
spells them. Extension methods use the spec's `_{name}` prefix, including
the bridge-internal prompt-response extension, which becomes
`_session/prompt_response`.

NATS subjects are unchanged. Subject tokens (`session.new`, `ext.{name}`,
bare `prompt` under the per-session agent marker) are routing addresses
governed by ADR 0003/0004, not protocol vocabulary; subject parsing is
untouched.

## Consequences

- No wire compatibility impact: message bytes on the NATS leg are identical
  before and after, because the method was never serialized. Peers upgrade
  independently.
- The conformance matrix compares method names against `meta.json` verbatim,
  with no translation table, and telemetry method labels match the spec.
- Decoding agent-bound requests via the SDK's `ClientRequest` enum becomes
  possible (evaluated separately; see PLAN.md Item 2).
- One extension-method spelling (`_{name}`) exists end to end.
