# Evaluating the "Harness-Driven Agents / Hyperlight micro-VM" article for Trogonai

**Date:** 2026-06-06
**Branches inspected:** `platform`, `platform-pro` (Rust monorepo under `rsworkspace/`)
**Source article:** *Harness-Driven Agents: Secure Podcast Pipeline in Hyperlight MicroVM Sandbox* (Microsoft Tech Community)
**Status:** Investigation complete. All load-bearing claims verified in code (file:line cited). Items I could not verify from the repo are listed explicitly in §10.

---

## 1. TL;DR

- **Do not adopt the article's solution** (Hyperlight / micro-VM as a code sandbox). It contains *untrusted code execution*, a problem Trogonai does not have where it matters — see §4–§6.
- **Take one idea from the article:** the *harness pattern* — an explicit chokepoint that **decides** before a dangerous action via a **capability allowlist + forbidden-action detection**. Re-aimed from "wrapping code" to "authorizing the autonomous agent's API actions," it addresses the one real, verified, high-relevance gap.
- **The real gap the investigation surfaced** (unrelated to the article's tech): the **autonomous agent (`trogon-agent`) takes irreversible write actions, driven by untrusted external input, with no runtime authorization.** This is the thing worth fixing.
- **Best solution is layered**, primary enforcement at the **secret-proxy chokepoint** (it is a verified *unavoidable* boundary), complemented by the agent-loop permission hook and founded on token least-privilege. See §8.

---

## 2. What the article proposes

The article describes a demo (`harnessagent_sandbox_demo`) built on **Hyperlight**, Microsoft's lightweight VMM (Apache-2.0; the core *Hyperlight* is a CNCF Sandbox project as of Feb 2025; `hyperlight-wasm` is the Wasm layer on top). Two ideas:

1. **"The harness decides, the micro-VM contains."** A policy/decision layer (the harness) plus a hardware-enforced containment layer (the micro-VM). The harness in the demo wraps generated code with: a URL allowlist, forbidden meta-tool detection, and timeouts.
2. **Snapshot + rewind per execution.** Capture a clean guest snapshot once; reset to it before every execution, making a many-step pipeline cheap and each run disposable.

Key facts about Hyperlight (verified): it is **experimental / not production-grade** (its own repo says so), it **does not run native bash** (it runs Wasm; the guest embeds wasmtime as a `no_std` module), and `hyperlight-wasm` is a real crate on crates.io.

---

## 3. Trogonai is two products in one repo

Verified by crate size, recent commit activity, and code inspection:

| | Interactive coding agent | Autonomous agent |
|---|---|---|
| Crates | `trogon-cli`, `trogon-*-runner`, `acp-nats*`, `trogon-wasm-runtime`, `trogon-runner-tools`, `trogon-agent-core`, `trogon-tools` | `trogon-agent` (biggest crate, ~31k LoC), `trogon-gateway` (source ingestion), `trogon-console`, `trogon-decider*`, `trogon-automations`, `trogon-vault*`, source connectors |
| Trigger | Human at the keyboard / WebSocket | External events (GitHub/Linear/Slack/CI/alerts) via NATS |
| Runs code? | **Yes** — bash via `trogon-wasm-runtime` | **No** — 15 API tools only |
| Human in loop? | Yes (permission system active) | **No** (`permission_checker: None`) |
| Recent dev momentum | High (compaction, spawn-agent, permissions) | Lower (large but stabilized) |

Note on activity: `trogon-agent` has the most *cumulative* churn (~700 file-touches over the branch) but is **absent from the last 40 commits**, where the interactive path dominates (`trogon-cli` 57, `trogon-runner-tools` 20, `trogon-acp-runner` 18). Cumulative investment ≠ current focus.

---

## 4. Verified findings — execution & containment

### 4.1 bash runs as a native process; "wasm" containment applies only to `.wasm` files
`trogon-wasm-runtime/src/runtime.rs:366` routes by extension: `let is_wasm = command.ends_with(".wasm")`. Only `.wasm` goes through the wasmtime sandbox (`run_wasm_terminal`); everything else (bash, git, etc.) falls to `process_spawner.spawn(...)` (`runtime.rs:395`) — a **native OS process**, contained only to a session directory (no kernel/hardware boundary). The crate name and `CLAUDE.md` ("WASM-based bash execution") oversell this. There is a `wasm_only` flag, but enabling it makes bash unrunnable (bash is not a `.wasm` file).

**Implication:** the article's "contain code in a micro-VM" gap is real *for the interactive path* — but that path is human-supervised and local / single-tenant-dedicated, so OS isolation + human approval already cover it.

### 4.2 The sub-agent `tools:` whitelist is parsed but never enforced
`.claude/agents/*.md` defines a `tools:` list (`trogon-runner-tools/src/subagents.rs:17`). It is consumed in exactly two places — neither enforces it:
- `trogon-acp-runner/src/agent.rs:496` (`spawn_subagent`) uses the def's `.model` and `.system_prompt`, **not `.tools`**.
- `agent.rs:734` uses `.name` only, to list agent names in the spawn tool description.

The sub-agent's toolset is built unconditionally from `all_tool_defs()` (`agent.rs:653`); the agent-core loop gates only via `permission_checker`, not via any tool-availability list (`trogon-agent-core/src/agent_loop.rs:924,1057`). So a spawned sub-agent receives the **full toolset** regardless of its `.md`. (`spawn_subagent` exists in `platform` only; `platform-pro` has a NATS cross-runner `SpawnAgentTool` instead.) It is **unenforced**, not merely honor-system. Note: sub-agents *do* get an isolated git worktree (`agent.rs:511`) and inherit the parent's mode/rules — so files are bounded, but per-role tool scoping is not.

---

## 5. Verified findings — hosted multi-tenant runs the *autonomous* agent, not interactive code

Determined from code, not docs:

- **Tenancy is enforced only on the autonomous side.** `tenant_id` is real (KV namespacing) in `trogon-agent` (`pr_review.rs:65,76`), `trogon-automations` (`runs.rs:85,98,135`), and the console route `/{tenant_id}/{session_id}` (`trogon-console/src/routes/sessions.rs:19`). The interactive path (`acp-*-runner`, `trogon-wasm-runtime`, `acp-nats-ws`, `trogon-runner-tools`) has **no `tenant_id` at all**, nor any synonym (`org`/`account`/`workspace`/`customer`/`user_id`).
- **The console exposes no interactive coding surface.** `trogon-console/src/server.rs` mounts exactly one router: `/sessions` (commented `// read-only`), `/agents` (+versions/rollback), `/skills`, `/environments` (+vault/credentials), `/mcp-registry`, `/-/health`. There is **no** prompt / new-session / WebSocket / bash endpoint.
- **`trogon-gateway` is source ingestion**, not interactive serving — all its routers are `source/*/server.rs` (telegram/twitter/github/sentry/slack/microsoft_graph webhook receivers).
- **Multi-tenancy is not prefix-based at the product level** either: `ACP_PREFIX` is a single env var (`trogon-acp-runner/src/main.rs:65`); per-tenant prefixes would be one runner instance per tenant (single-tenant deployments), not shared multi-tenant.

**Conclusion:** the hosted multi-tenant offering = autonomous agent (tenant-scoped API actions) + read-only management console. The interactive code agent is deployable and reachable over a (tenant-less) WebSocket, but is **not** wired into the multi-tenant product. So "untrusted code on shared multi-tenant infra" — the only scenario where the article's micro-VM is genuinely required — **is not Trogonai's model today.**

---

## 6. Verified findings — the real, high-relevance gap: autonomous actions are unauthorized

This is the gap that matters, and the article does **not** address it.

1. **No human gate.** Autonomous handlers pass `permission_checker: None` (`trogon-agent/src/handlers/{pr_review,issue_triage,push_to_branch,alert_triggered,...}.rs`; `trogon-agent/src/agent_loop.rs:2604,2713,...`). The loop *does* honor a checker if present (`agent_loop.rs:2249-2253`) — it is simply never set.
2. **Untrusted external content enters the prompt verbatim** (the prompt-injection vector, verified end-to-end):
   - `handlers/comment_added.rs:30` reads `event["comment"]["body"]` and interpolates it raw into the prompt (`:44-45`).
   - `handlers/issue_triage.rs:44,50` injects the issue `title`.
   - `handlers/pr_review.rs` instructs the agent to pull PR content/comments.
3. **Destructive tools with no scoping.** `update_file` writes to a **model-chosen branch defaulting to `main`** (`tools/github.rs:209`); `owner`/`repo` are also model-chosen. There is **no repo/org allowlist** anywhere (`trogon-agent`, `trogon-gateway` grep empty).
4. **The secret-proxy pins host but not action.** `trogon-secret-proxy/src/proxy.rs:44-79` routes `/{provider}/{*path}`, builds the upstream from a fixed `provider::base_url()` (github→api.github.com, slack→slack.com/api), and 502s unknown providers. So a token **cannot** be redirected to an arbitrary host (no arbitrary exfiltration) — but **within** a provider there is no path/method/repo/branch restriction.

**End-to-end attack path (verified):** external comment/issue text → autonomous agent prompt (verbatim, unsanitized) → `permission_checker: None` → write tool (e.g., `update_file` to `main`) → proxy forwards to api.github.com. The only backstops are **token scope and GitHub branch protection — both external to the repo and never checked in code.**

### Severity
**Bounded but real.** Host pinning prevents arbitrary-host exfiltration (an earlier over-statement of mine — corrected). The remaining exposure is *intra-provider* unauthorized writes (commit-to-main on any reachable repo, Slack posts, Linear edits). Whether this is "noisy" or "serious" depends on the GitHub token's scope and branch protection — **not verified** (ops config, outside the repo). A mitigating factor: each handler scopes its own toolset (`comment_tools()`, etc.), so the worst case is a handler that both ingests raw external text **and** exposes a write tool — the exact per-handler mapping was not fully enumerated (§10).

---

## 7. Verdict on the article

**Do not adopt** the Hyperlight / micro-VM solution:

1. **Solves a problem we don't have where it matters.** Hosted multi-tenant runs no code (§5); the code-running path is local/single-tenant + supervised (§4.1).
2. **Architectural mismatch.** The article pulls the boundary *into* the agent process ("sandbox as a library call"); Trogon deliberately pushed execution *out* to a separate NATS service (`trogon-wasm-runtime`).
3. **The tech is not adoptable now.** Hyperlight is experimental and does not run native bash — it cannot contain the one thing that runs code today.
4. **Opportunity cost.** The real, present, higher-severity gap is autonomous action authorization (§6), which the article does not touch.

**Reactivation trigger to record:** revisit only if the product decides to run **tenant code on shared multi-tenant infra**. Until then there is no case.

**The one idea worth taking:** the *harness pattern* (capability allowlist + forbidden-action detection + chokepoint that decides), re-aimed from wrapping *code* to authorizing the autonomous agent's *API actions*. Also worth capturing: the "decide vs contain" framing as a design tenet — the gateway design doc currently *assumes* execution isolation is "already covered by `trogon-wasm-runtime`," which §4.1 shows is false.

---

## 8. Recommended solution (layered, priority-ordered)

The autonomous gap is an **authorization** problem, not a compute-sandbox problem. Defense in depth:

1. **Foundation — token least-privilege + branch protection (ops/config).** If the GitHub/Slack/Linear tokens cannot do damage, prompt-injection is harmless regardless of what the model decides. Cheapest, strongest, external. **Verify current scopes first** — it determines urgency.
2. **Primary enforcement — action authz at the secret-proxy chokepoint (the article's harness idea).** The proxy is a **verified unavoidable boundary**: the agent holds only opaque `tok_` placeholders (`config.rs:11-18`), the proxy requires `tok_` and resolves against the vault (`worker.rs:21`), and the agent does **not** read the vault directly (its `trogon-vault` dep is only for the `ApiKeyToken` type). Therefore no authenticated call can avoid the proxy. Extend the existing provider allowlist (`proxy.rs`) into a **repo/org allowlist + protected-branch block + method/path rules**. This is the strongest layer precisely because it **cannot be forgotten** — unlike the per-handler checker, whose failure mode (`None` everywhere) is the current bug.
3. **Complement — wire the agent-loop `permission_checker`.** It already exists and is honored (`agent_loop.rs:2249`) but is passed `None`. A checker here has richer semantics (it sees `update_file{branch:"main"}` structurally, no HTTP parsing). Good for fine-grained policy; weaker as the sole control because it relies on per-handler diligence.
4. **High-impact actions — human approval via `trogon-vault-approvals`.** The approval/Slack-notify plumbing exists (today for *credential provisioning*, in `trogon-secret-proxy`); the per-action gate would be new logic but can reuse that plumbing.
5. **Cheap adjacent fix — enforce the sub-agent `tools:` whitelist** (§4.2), independent of the above.

**Why the proxy is primary (and why I initially got this wrong):** an early draft ranked the proxy as a 4th-tier "defense in depth." That was a mistake. For a security control on an *autonomous* agent acting on *untrusted* input, an **unavoidable, centralized** chokepoint (one policy, all agents/paths, impossible to forget) beats a semantically nicer but forgettable per-handler hook — especially since the current vulnerability *is* the forgotten per-handler hook. Its one weakness (it inspects raw HTTP, so branch rules require parsing the request body) is an implementation cost, not a disqualifier.

---

## 9. Corrections made during the investigation (for the record)

This investigation was iterative; several confident claims were revised once checked. Recording them so the conclusions are auditable:

- **"bash is sandboxed by the wasm runtime"** → false; bash is a native process (§4.1).
- **"sub-agent tool filtering is honor-system"** → understated; it is **unenforced** (§4.2).
- **"the gateway doc anticipates deployment-boundary execution isolation"** → false; the doc explicitly scopes execution isolation *out* and *assumes* it is already covered, which it is not.
- **"trogon-agent is the priority because it's the most active"** → mis-measured (cumulative vs recent churn); recent focus is the interactive path (§3).
- **"approvals already gate agent actions"** → false; `vault-approvals` gates *credential provisioning*, not agent actions (§8.4).
- **"the secret-proxy doesn't restrict host"** → false; it pins host per provider (§6.4) — this *reduced* the severity I had asserted.
- **"the proxy harness is a 4th-tier layer"** → wrong; it is the strongest primary enforcement point because it is unavoidable (§8).

---

## 10. Not verified (open items, mostly ops/config outside the repo)

- **GitHub/Slack/Linear token scopes** and **branch protection** on target repos — determines whether §6 is "noisy" or "serious."
- **Per-handler tool exposure** — exact mapping of which autonomous handlers ingest raw external text *and* expose write tools (`update_file`/`create_pull_request`). Needed to pin worst-case severity per route.
- **The article body itself** — could not be fetched (page returns only the title to fetchers); article claims are reconstructed from search indexing + the author's parallel write-up. Hyperlight facts are from Microsoft primary sources.

---

## 11. Recommended next steps

1. Confirm token scopes + branch protection with ops (cheap; sets urgency).
2. Map per-handler tool exposure for the autonomous agent (cheap; pins severity).
3. Implement repo/org allowlist + protected-branch block at the secret-proxy (`proxy.rs`); wire high-impact actions to `vault-approvals`.
4. Set the autonomous agent-loop `permission_checker` to a real checker (stop passing `None`).
5. Enforce the sub-agent `tools:` whitelist (§4.2).
6. Record the "decide vs contain" tenet and the Hyperlight reactivation trigger; otherwise park the article.
