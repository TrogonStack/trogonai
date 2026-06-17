# Codex Runner → Full Parity Plan

**Goal:** bring `trogon-codex-runner` to operational parity with the xAI, OpenRouter, and ACP runners, fixing every defect and gap surfaced by the 2026-06-17 verification workflow (17-agent audit, all 11 claims confirmed + additional gaps found).

**Branch:** `fix/codex-full-parity` off `feat/cli-fixes`.

**Scope note — what "full parity" can and cannot mean.** Codex drives an external `codex app-server` subprocess (like acp-runner drives `claude`); xAI/OpenRouter own their tool loop in-process. Most gaps are *closable* and reach true parity. A small set are bounded by the subprocess architecture and are addressed as "best-effort + honest refusal," not silent pretense:

- **Per-tool permission gating / elicitation** — *possibly* closable against the **real** codex app-server (it sends `execCommandApproval` / `applyPatchApproval` server→client requests that the current `read_loop` ignores). Feasibility is settled in **Phase 0** against the real schema, implemented in Phase 7 if confirmed.
- **Plan mode (read-only)** — intentionally refused; cannot be enforced when tools run in-subprocess. Stays refused.
- **Multimodal/image prompt content** — depends on the real app-server's input schema; currently dropped. Phase 0 reads the real input schema and decides.

---

## Critical risk — and why it is now cheaply de-riskable

The historical risk: project memory (2026-06-10 live test) records that the runner's `thread/start → threadId` protocol **matched only its own mock**, not real codex `0.133/0.138`. The runner + its `mock_codex_server.rs` were written from the same assumptions, so the integration tests pass *by construction* and prove nothing about the real wire. Every fidelity fix below (usage, reasoning, tool events, cumulative-text, approvals) is shaped by an assumption about that wire.

**What changed (2026-06-17):** the real binary and credentials are on this machine, and the binary can emit its own protocol:

- **`codex-cli 0.138.0`** installed at `/home/jorge/.nvm/versions/node/v22.22.0/bin/codex` — *exactly* one of the versions memory flagged as mismatched, i.e. the known-divergent binary, which is ideal for finding the gap.
- **`OPENAI_API_KEY` + `CODEX_*`** present in `rsworkspace/.env.local`, so real turns can be driven end-to-end.
- The runner spawns `codex app-server` (`process.rs:117-118`) — the **same entrypoint** the real 0.138 binary exposes, so there is no entrypoint mismatch; the divergence is purely message shapes.
- **`codex app-server generate-json-schema`** (and `generate-ts`) emit the **authoritative protocol schema** for 0.138. We do not have to reverse-engineer the wire — codex hands it to us.

**Consequence:** the "mock vs real" risk — which was the entire source of the long tail — collapses into a cheap, deterministic **Phase 0** spike. We generate the real schema, diff it against the runner's assumptions, run a few real probe turns for runtime behavior the schema can't express (notably whether `item/updated` is a cumulative snapshot or a true delta — B2's premise), and then write every later fix against ground truth the first time. Phase 7 demotes from "high-variance discovery" to "run the live suite against shapes we already validated."

**The one thing Phase 0 may surface:** if 0.138's schema differs *substantially* from what `process.rs::parse_event` / `traits.rs` assume (method names, `thread` vs `conversation` semantics, event envelopes), there is a **foundational protocol-bump** task that must land before any fidelity work — and that is the most likely real reason codex is stuck at "observational-only." Phase 0 finds this on day one instead of mid-Phase-4.

---

## Scoping — you do NOT have to do all of it

Full parity (all phases) is **~4.5–5.5 weeks**, but most of that cost lives in two phases that are deferrable for a secondary/observational runner. Pick a **tier**, not the whole campaign.

### Delivery tiers
| Tier | Includes | Covers | Calendar (1 dev) |
|------|----------|--------|:----------------:|
| **T0 — Spike only** | Phase 0 | Decision data; no product change | ~2–3 days |
| **T1 — Stop the bleeding** | Phase 0 + B1/B2 + per-request timeout (R1) + honest errors (R6-lite) | Codex no longer hangs, dup-streams, or lies on cancel | **~1 week** |
| **T2 — Daily-usable (recommended)** | T1 + `/compact` (E1) + usage→`/cost` (P5) + auth (C1/C2) | **Every user-visible gap from the audit** | **~2 weeks** |
| **T3 — Full parity** | + persistence (5) + full fidelity (4) + live suite (7) | True parity incl. session-survives-restart | ~4.5–5.5 weeks |

**T2 retires ~90% of what a user would actually notice for ~⅓ the time.**

### What gets deferred by default (and why it's OK)
- **Phase 5 — Persistence (~4 days, the single biggest chunk):** only matters if codex becomes a **daily driver**. It is currently observational-only / RUN-2 descoped. Sessions-survive-restart is nice-to-have for a secondary runner. **Defer until codex is promoted.**
- **Phase 7 — Live validation suite (~2 days):** Phase 0 already establishes the real-wire truth; a full `#[ignore]` suite is belt-and-suspenders. **Defer.**
- **Phase 4 — trim to usage-only:** ship `/cost` (P5); skip reasoning/tool-kind polish (P3/P4) until there's demand.

### Board & estimate posture
- **Keep this plan as a repo doc, not a board epic.** Put on the board only the tier actually committed to — at minimum, nothing larger than the **T0 spike**.
- **No public estimate until after Phase 0.** Phase 0 (~2–3 days, ~$0.10) converts the biggest unknown into a measurement; *then* a tier estimate (e.g. "T2 ≈ 2 weeks") is a number worth standing behind.
- **Recommended path:** commit to **T0 only** now → run Phase 0 → choose T1/T2/T3 from data.

---

## Phase 0 — DONE (2026-06-17) · verdict

Ran against real **`codex-cli 0.138.0`**. Full findings: **`docs/codex-protocol-delta.md`**; authoritative schema committed under **`docs/codex-protocol-0.138/`**.

- **Verdict: a foundational protocol bump IS required (new Phase 0.5).** The runner's `process.rs::parse_event` keys streaming on `item/updated`, **which does not exist** in 0.138. The real event model (`item/agentMessage/delta`, `item/started`/`completed`, `item/reasoning/*`, `thread/tokenUsage/updated`) and the handshake (`thread.id`/`cwd`, not `threadId`/`workingDirectory`; `turn/start.input` is an array) all differ. The mock + integration tests encode the wrong protocol and must be rebuilt.
- **Big upside:** native methods exist for **mid-turn steering (`turn/steer`), compaction (`thread/compact/start`), token usage (`thread/tokenUsage/updated`), per-tool approvals + elicitation (`ServerRequest`: `execCommandApproval`, `applyPatchApproval`, …), MCP, skills, and thread persistence/resume (`thread/list`/`read`/`resume`)**. Several gaps collapse from "build it" to "wire the native method." **Per-tool gating flips from architectural-infeasible to feasible.**
- **B2 is moot** (the cumulative-snapshot premise was wrong); **P3/P5** have exact notifications identified.
- **Revised single-dev estimate for real-codex parity: ~5–6 weeks** — the speculative tail is replaced by the concrete Phase 0.5 rewrite, roughly offset by the native-method savings in later phases.

### Phase 0.5 — Foundational protocol bump (NEW, now the gate)
Rewrite `process.rs` to the real 0.138 wire: `thread_start` → read `thread.id`, send `cwd`; `turn_start` → `{threadId, input:[…]}` array; `parse_event` → real notification set; `read_loop` → inbound-`ServerRequest` branch + `turnId`/`itemId` keying; new `CodexEvent` variants (AgentMessageDelta, ReasoningDelta, ItemStarted/Completed, TokenUsage, ApprovalRequest). Rebuild `mock_codex_server.rs` + integration tests against the committed schema. **Gates Phases 1–7.** Est: ~3–5 ideal days.

---

## Workstreams

### Phase 0 — Protocol ground-truth spike (DONE — see verdict above)
De-risks every protocol-dependent fix below by replacing assumptions with the real 0.138 schema and live traces. Near-zero cost (schema gen is free/local; probe turns are a few cents of OpenAI tokens).

| ID | Task | Approach |
|----|------|----------|
| **PG-1** | Capture the authoritative protocol | Run `codex app-server generate-json-schema` (+ `generate-ts`) and commit the output under `docs/codex-protocol-0.138/`. This is the source of truth for all later phases. |
| **PG-2** | Diff real schema vs runner assumptions | Compare the schema against `process.rs::parse_event` (method names, `item/updated`/`item/completed`/`turn/completed`/`token_count`/`reasoning`/approval requests), `traits.rs` (`thread_start`/`turn_start` shapes), and `mock_codex_server.rs`. Produce `docs/codex-protocol-delta.md`. |
| **PG-3** | Live runtime trace | With `OPENAI_API_KEY`, drive 2–3 real turns through `codex app-server` (a small probe harness or the runner with `RUST_LOG=debug`). Capture actual event ordering and settle: is `item/updated` cumulative or delta (B2)? what does `token_count` carry (P5)? are `reasoning` items emitted (P3)? are `execCommandApproval`/`applyPatchApproval` requests sent (per-tool gating)? |
| **PG-4** | Re-scope decision | From the delta, decide: (a) is a foundational protocol-bump of `process.rs` needed before fidelity work? (b) is per-tool approval gating feasible? (c) is image input supported? Fold answers back into Phases 1–7. |

**Est: ~0.5–1 day.** **Output gates everything else.** If PG-4 finds a large protocol bump, insert it as Phase 0.5 (est. revised then).

### Phase 1 — Real bugs
| ID | Defect | Files | Approach |
|----|--------|-------|----------|
| **B1** | TROGON.md double-injected on first turn | `agent.rs:646-659` | *Protocol-independent — safe to do even before Phase 0.* The `else if first_turn` branch and the unconditional `if prepend_trogon` branch both prepend `load_trogon_md`. Collapse to a single injection; tighten the regression test (`trogon_md_injection_bug.rs:110-119`) from `.contains()` to an occurrence count == 1. |
| **B2** | Cumulative-text re-emission → duplicated streaming | `process.rs:411-428`, `agent.rs:737-748` | *Premise confirmed by PG-3.* If `item/updated` is a cumulative snapshot: track per-item emitted length and emit only the suffix as `AgentMessageChunk`; reconcile on `item/completed`. Update the mock to re-send cumulative snapshots so the test exercises the real behavior. (If PG-3 shows true deltas, this shrinks to a no-op + a guard.) |

**Est: ~1 day** (B1 ~0.25, B2 ~0.75, contingent on PG-3).

### Phase 2 — Capability/auth surface (cheap, client-discoverability)
| ID | Gap | Files | Approach |
|----|-----|-------|----------|
| **C1** | `initialize()` advertises no `auth_methods` | `agent.rs:347-363` | Advertise an `AuthMethod::EnvVar(OPENAI_API_KEY)` (mirror xai's env-var method) so clients can discover auth even though the key is env-sourced. |
| **C2** | `authenticate()` accepts any/empty method id | `agent.rs:366-372` | Validate `req.method_id` against the advertised method; reject unknown ids (mirror the acp-runner fix already landed in `0883d8b1`). |
| **C3** | `prompt_capabilities(embedded_context)` / `branchAtIndex` | `agent.rs` | **Decision, mostly no-op:** leave `embedded_context` *off* unless Phase 7 enables embedded content (advertising it while dropping non-text blocks would be a false claim). `branchAtIndex` stays off (codex owns thread history; index-truncation not expressible). Document both as intentional. |

**Est: ~0.5 day.**

### Phase 3 — Robustness (subprocess hardening)
| ID | Gap | Files | Approach |
|----|-----|-------|----------|
| **R1** | No per-request JSON-RPC timeout (can hang forever) | `process.rs:276-291` | Wrap the `rx.await` in `tokio::time::timeout` (env `CODEX_REQUEST_TIMEOUT_SECS`, sane default). Remove the pending entry on timeout. |
| **R2** | Prompt-timeout doesn't interrupt the app-server turn | `agent.rs:715-719` | On timeout, call `turn_interrupt` before breaking so the subprocess stops doing leaked work. |
| **R3/P6** | Cancel never reaches the loop; no `StopReason::Cancelled`/`MaxTurnRequests` | `agent.rs:585-862, 866-887` | Thread a cancel channel/`Notify` into the prompt loop `select!`; return `StopReason::Cancelled` on cancel, distinguish timeout. Mirror openrouter (`agent.rs:1927-2071`). |
| **R4** | `turn_interrupt` failures swallowed but return `Ok` | `agent.rs:878-886` | Surface the failure (typed error / propagate), optionally one retry. |
| **R5** | No retry/backoff on transient spawn/handshake/turn errors | `agent.rs:277-281, 694-703` | Bounded retry for spawn + handshake (HTTP-status retries N/A for subprocess; transient spawn is). |
| **R6** | Errors collapse to opaque `String` (violates typed-error convention) | `process.rs:288-290, 344-350, 386-395`, `agent.rs:856-859` | Introduce `CodexError` enum (ProcessDead, Timeout, RpcError{code,message}, TurnError, Spawn) implementing `Display + Error`; map app-server JSON-RPC error objects to variants. Removes the `String` channels in `PendingMap`/`request`. |

**Est: ~2 days** (R6 is the bulk; R3 overlaps Phase 5's loop changes).

### Phase 4 — Prompt-loop fidelity
| ID | Gap | Files | Approach |
|----|-----|-------|----------|
| **P5** | No token usage → `/cost` blank, compactor blind | `process.rs:51-66, 398-462`, `agent.rs` | Add `CodexEvent::Usage{input,output,...}`; parse the app-server `token_count`/turn-usage notification; emit `SessionUpdate::UsageUpdate`. (Real-protocol-shape dependent → validate Phase 7.) |
| **P3** | Reasoning/thoughts dropped | `process.rs:454,467`, `agent.rs` | Map `reasoning` items → `CodexEvent::Thought` → `SessionUpdate::AgentThoughtChunk` (mirror acp `prompt_converter.rs:81-82`). |
| **P4** | Tool-call fidelity: hardcoded `Execute`, always `Completed`, no locations/Failed | `agent.rs:754,771-784` | Map tool kind from item type (Read/Edit/Execute/Search/Fetch), derive `Failed` from exit_code/error, attach `locations`/`meta`, structured output blocks. Mirror acp `tool_kind_for`. |
| **P2** | No PLAN_MODE / URL_FETCH guidance injection | `agent.rs:664-669` | Prepend `URL_FETCH_GUIDANCE`; when `mode=="plan"`-equivalent context applies, the codex refusal stands, but add behavioral guidance parity where modes are honored. |
| **P8** | Console-KV skills not injected (no system prompt) | new `skill_loader` wiring | *Optional/closable:* inject console skills as a first-turn user-prepend (same mechanism as TROGON.md). Decide whether worth it given codex's no-system-prompt model. |

**Est: ~2.5 days** (P5+P3+P4 are the substance; P2 small; P8 optional).

### Phase 5 — Session persistence (the big one)
The runner's own docs call this "architecturally infeasible." **The audit refutes that:** xai is *also* provider-stateful (`previous_response_id`) yet persists a local history mirror to KV and replays it on restart. Codex already keeps `history: Vec<PortableMessage>` (`agent.rs:111`) **and** the `pending_history` replay path (`agent.rs:639-645`). Only the store + restore wiring is missing.

| ID | Gap | Files | Approach |
|----|-----|-------|----------|
| **S1** | Sessions in-memory only; lost on respawn | `agent.rs` struct/new, `main.rs` | Add a session store to `CodexAgent` (field + constructor + `main.rs` wiring, mirror xai `with_default_session_store`). Map `CodexSession ↔ SessionState`: `SessionState.history` carries the PortableMessage mirror; thread_id/cwd/mode/model/parent stored in a codex sub-struct serialized into the snapshot (decide: extend `SessionState` vs dedicated codex KV bucket — **recommend dedicated `CODEX_SESSIONS` bucket keyed by session id** to avoid polluting the shared `SessionState`). Persist on new/prompt-commit/fork/set_mode/set_model. |
| **S2** | `load_session`/`resume` no KV fallback | `agent.rs:408-450` | After the in-memory miss, restore from store: rebuild `CodexSession`, set `pending_history = history`, `first_turn = true`, and start a **fresh** codex thread (old thread_id is dead after respawn) so the next turn replays the compacted/restored history. |
| **S3** | Fork not persisted; lineage lost | `agent.rs:474-491` | `store.save` the forked snapshot incl. `parent_session_id` (mirror openrouter `agent.rs:1396-1398`). |
| **S4** | Codex sessions invisible to trogon-console | `main.rs` | Open the SESSIONS bucket / wire `AGENT_ID` so sessions show in console; enables S1 list-from-KV for `list_sessions`. |

**Est: ~2.5–3 days** (the `CodexSession ↔ store` mapping + restore-replays-fresh-thread is the design-heavy part; comes with kv integration tests).

### Phase 6 — `session/compact` + mock/tests
| ID | Gap | Files | Approach |
|----|-----|-------|----------|
| **E1** | `session/compact` → MethodNotFound; `/compact` broken on codex | `agent.rs` ext_method + struct | Wire a compactor NATS client into `CodexAgent` (field + `main.rs`, mirror xai `with_compactor`). Add the `session/compact` arm: export local history → `maybe_compact` → replace `history`, set `pending_history`+`first_turn` so the next prompt replays compacted history on a fresh thread. Depends on Phase 5's replay mechanism. |
| **T1** | Mock doesn't model usage/reasoning/approvalPolicy/cumulative-text/Failed | `bin/mock_codex_server.rs` | Extend mock: emit `token_count`, `reasoning` items, cumulative message snapshots, tool-failure, and a `MOCK_REQUIRE_APPROVAL_POLICY` flag asserting `approvalPolicy` reaches `turn/start` (mirror `MOCK_REQUIRE_MODEL`). |
| **T3** | Missing test-category breadth | `tests/` | Add `kv_integration.rs` (Phase 5), `compaction_integration.rs` (E1), approvalPolicy-on-wire assertion. |

**Est: ~1.5 days** (T1 mock work is reused by Phases 3–4 verification).

### Phase 7 — Live parity validation (de-risked by Phase 0)
| ID | Gap | Approach |
|----|-----|----------|
| **T2** | No live test against a real codex | Add an `#[ignore]`-gated live suite (mirror xai `live_permissions.rs`) driving `codex-cli 0.138.0` with `OPENAI_API_KEY`. Because Phase 0 already captured the schema and reconciled `parse_event`/`thread_start`/usage/reasoning/tool shapes, this is **validation, not discovery** — assert the live wire matches what Phases 1–6 built against. |
| **T4** | Per-tool approval gating (conditional on PG-3) | *Only if Phase 0 confirmed it:* handle inbound `execCommandApproval`/`applyPatchApproval` server→client *requests* in `read_loop` (currently it routes only responses + notifications) and bridge them to Trogon's elicitation/permission path — flipping the one "architectural-infeasible" item. Skipped if PG-3 shows no such requests. |

**Est: ~1.5–2.5 days** (down from 2–4 — discovery moved to Phase 0). If per-tool approvals are confirmed and wired, add ~1–2 days.

---

## Items that stay non-parity (document, don't "fix")
- **Plan mode read-only enforcement** — refused by design (`permissions.rs:51-57`). Stays.
- **Per-tool gating** — pursued only if Phase 0 (PG-3) confirms the real app-server sends approval requests; otherwise coarse `approvalPolicy` is the boundary.
- **`branchAtIndex`** — codex owns thread history; not expressible. Stays off.
- **Multimodal/image input** — decided in Phase 0 against the real input schema; if unsupported, stays dropped.

---

## Estimate summary (single dev, realistic)

These are **ideal focused engineering days** — uninterrupted heads-down work, *not* calendar days. The conversion to calendar is below and is where the real number lives.

| Phase | Content | Ideal days | Confidence |
|-------|---------|:----------:|:----------:|
| 0 | Protocol ground-truth spike (schema gen + diff + live trace) | 0.5–1 | high |
| 1 | Real bugs (TROGON.md dup, cumulative-text streaming + mock) | 1 | high |
| 2 | auth_methods, authenticate validation, cap decisions | 0.5 | high |
| 3 | Robustness — timeouts, cancel/stop-reasons, retry, **typed `CodexError`** (ripples through `PendingMap`/`request`/`read_loop`) | 2.5–3 | medium |
| 4 | Prompt-loop fidelity — usage, reasoning, tool events, guidance | 3 | med-high¹ |
| 5 | Session persistence + restore-on-fresh-thread + fork/list/resume + KV tests | 3.5–4 | low-med |
| 6 | `session/compact` + compactor wiring + mock/test extensions | 2 | medium |
| **Subtotal (0–6)** | **parity, built against the real schema** | **~13–14.5** | |
| 7 | Live validation suite (+ per-tool approvals if PG-3 confirms) | 1.5–2.5 | medium |
| **Total (real-codex parity)** | | **~14.5–17 ideal days** | |

¹ Phase 4's confidence rises from *medium* to *med-high* because Phase 0 replaces its wire assumptions with the captured schema. The remaining uncertainty is in **PG-4's verdict**: if Phase 0 finds a large protocol bump (a Phase 0.5), add **2–4 ideal days** and re-baseline.

### Why calendar ≠ ideal days for one dev
- **No parallelism.** Everything is strictly serial — there is no Lane A/Lane B.
- **Build/test friction is real here.** `cargo` must be serialized against the shared target, full rebuilds can OOM, the build disk is tight, and integration/KV tests need a live NATS. Every change pays a slow build+clippy+test cycle.
- **`warnings = deny` + `clippy::all = deny`** globally — no "fix it later"; each commit must be green.
- **Phase 5 is still discovery work.** Restore-replays-on-a-fresh-thread has subtle correctness (the old `thread_id` is dead after respawn). Phase 0 removes the *protocol* uncertainty, but the persistence design work remains.

A single dev realistically converts **~3.5–4 ideal days per calendar week** in this environment (the rest goes to builds, debugging, the deny-gate, and context-switching).

### Realistic calendar
| Target | Ideal days | Calendar (1 dev) |
|--------|:----------:|:----------------:|
| **Phase 0 alone (decision point)** | 0.5–1 | **~2–3 days** |
| **Core parity (Phases 0–6)** | 13–14.5 | **~4 weeks** |
| **Full real-codex parity (Phases 0–7)** | 14.5–17 | **~4.5–5.5 weeks** |

Treat these as **P50**. The big change from the pre-Phase-0 estimate: **the ~7-week tail is largely gone** — building against the real 0.138 schema removes the late-discovery rework that drove it. The residual risk is concentrated in **Phase 0's verdict**: if PG-4 finds a foundational protocol bump, add 2–4 ideal days (~1 calendar week) and the total moves toward ~6 weeks. Either way, **Phase 0 converts the project's biggest unknown into a day-one, ~$0.10 measurement** — run it before committing to the full timeline.

**Recommended order:** **0 → (B1 anytime) → 1 → 2 → 3 → 4 → 6 → 5 → 7.** Phase 0 first because its output (the real-protocol delta + PG-4 verdict) re-baselines every later phase; B1 is protocol-independent and can land in parallel as a quick win. Persistence (5) after the loop work so the snapshot captures the final session shape; Phase 7 is now validation rather than discovery.

**Honest bottom line:** for one dev, plan on **~4 weeks to parity built against the real 0.138 schema** and **~4.5–5.5 weeks to live-validated real-codex parity** — with a **~6-week** outcome only if Phase 0 surfaces a foundational protocol bump. The pre-Phase-0 "~7-week tail" is largely retired because we now build against ground truth instead of the mock. **Do Phase 0 first** (~2–3 calendar days, ~$0.10): it's the cheapest possible way to confirm the whole estimate before committing.
