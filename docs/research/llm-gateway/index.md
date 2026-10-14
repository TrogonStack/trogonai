# LLM gateway research corpus

This corpus is the research input behind the platform's model access path:
how production gateways put one endpoint between callers and many model
providers, what the records behind that endpoint own (routes, credentials,
keys, tenants, budgets, limits, guardrails, caches, prompts), where their
configuration binds, and which of those responsibilities this repository has
already decided or still owes. Where a conclusion here differs from a record
in the [ADR index](../../adr/index.md), the ADR is authoritative.

## Method

Each dossier quotes its primary source, dates the retrieval, and marks every
inference as ours. Products are studied through their published
documentation; when a documentation site is a rolling build, the dossier pins
by retrieval date and whatever version the pages declare, and says the pin is
weak. Contradictions between upstream pages are recorded with both pages
cited rather than resolved. Gateway material already captured in other
corpora is linked rather than restated.

## Product dossiers

- [LiteLLM Proxy](./products/litellm.md)

Evidence on the same product held in neighbouring corpora:

- [LiteLLM MCP Gateway](../mcp-gateway/products/litellm-mcp-gateway.md), the
  MCP side of the same proxy.
- [LiteLLM as an agent platform](../agent-platform/products/litellm.md).
- [LiteLLM Proxy provider contracts](../provider-agent-contracts/products/litellm-proxy.md),
  the request and response surface it translates.

## ADR pointers

- [ADR#0032: Model Route and Credential Binding](../../adr/0032-model-route-and-credential-binding.md)
  (draft). Exact routes and credential bindings per Session, no aliases,
  failover, or credential probing, and budget and rate enforcement placed at
  the model access service. This is the record most gateway reliability
  features diverge from.
- [ADR#0069: Every Event Envelope Records Who Caused It and When Each Step Happened](../../adr/0069-event-envelope-records-issuer-and-times.md)
  (draft). A requester-written value is a claim, not a record; relevant to
  caller-supplied attribution fields.
- [ADR#0046: Project-Anchored Resource Hierarchy](../../adr/0046-project-anchored-resource-hierarchy.md).
  Organization over project, with the project owning credentials and quotas.
- [ADR#0007: Configuration Sources](../../adr/0007-configuration-sources.md).
  Configuration precedence with no database overlay layer.
- [ADR#0023: Secret Management and Key Custody on OpenBao](../../adr/0023-secret-management-and-key-custody-direction.md).
- [ADR#0050: Signed Proof-of-Possession Caller Authentication](../../adr/0050-signed-first-caller-authentication.md).
- [ADR#0026: Command Authorization Principal](../../adr/0026-command-authorization-principal.md).
- [ADR#0028: Admission Control for Decider Command Execution](../../adr/0028-decider-admission-control-and-backpressure.md).
- [ADR#0008: OpenTelemetry Observability](../../adr/0008-opentelemetry-observability.md)
  and [ADR#0042: NATS Trace Context](../../adr/0042-nats-trace-context-and-message-path-tracing.md)
  (draft).
- [ADR#0021: Typed Decode over Passthrough Forwarding](../../adr/0021-typed-decode-over-passthrough-forwarding.md),
  by analogy for pass-through endpoints.
- [Usage Settlement Ledger](../../architecture/usage-settlement-ledger.md),
  for where settlement lives relative to gateway spend records.

## Open questions

- Budget and rate policy. ADR#0032 places enforcement at the model access
  service and leaves the product policy open. Which budget scopes, windows,
  and fail-closed behaviours that service owes is undecided.
- Gateway as route executor. Whether an off-the-shelf gateway can run behind
  ADR#0032 with substitution features disabled, and how that configuration
  would be verified, is open.
- Caller-supplied attribution. Which request fields, if any, the model access
  path should accept as labelled claims for spend, caching, and tracing is
  open.
- Breadth. Conclusions about model gateways in general wait on dossiers for
  more products.
