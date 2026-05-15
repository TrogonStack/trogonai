# Trogon — Next Priorities

The vault work is complete and merged into `platform`. Here is what comes next, in recommended order.

---

## 1. Token & Cost Tracking — Low Effort, High Value

Every response from Claude includes how many tokens were used. Right now trogon throws that number away.

Without this, there is no way to know which agents, sessions, or tenants are consuming the most resources. Adding it means storing those numbers in the existing transcript — the structure is already there. Estimated effort: one afternoon.

---

## 2. Streaming Responses — Medium Effort, Critical UX

When a user sends a message to an agent, they see a blank screen until the agent finishes its entire response. Foundry streams tokens to the client in real time.

For trogon this means opening a streaming channel from the worker back to the HTTP proxy so the response arrives progressively. This is the most visible UX gap for any interactive use case. Estimated effort: 2–3 days.

---

## 3. Tool Approval Gates — Medium Effort, Security Critical

If an agent is about to do something sensitive — merge to production, delete data, post to a Slack channel — there is currently no way to pause and ask a human for confirmation before it runs.

Foundry has this as a built-in permission callback. In trogon the underlying pattern already exists (we use it for credential approvals in the vault). Extending it to tool calls is a matter of applying the same mechanism. Estimated effort: 2–3 days.

---

## 4. Deploy the Multi-Agent System — Low Code, Medium Ops

Trogon already has the full multi-agent architecture implemented: agents discover each other, get routed dynamically by an LLM router, spawn sub-agents, and share an append-only audit trail across hops. All of this code is in `platform` today.

The gap is not code — it is that none of it has been deployed or wired together in production yet. This is the largest architectural investment already made, and it is sitting unused.

---

## Recommended Order

| Priority | Feature | Estimated Effort |
|---|---|---|
| 1 | Token & cost tracking | 1 afternoon |
| 2 | Streaming SSE | 2–3 days |
| 3 | Tool approval callbacks | 2–3 days |
| 4 | Deploy multi-agent system | Medium ops work |
