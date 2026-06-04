# Trogon Front-End: Full Proposal

> Status: draft proposal. Grounded in the `platform` branch (routes, models, proto
> aggregates, and crates as they exist there). Design method: Event Modeling.

## 1. Thesis

### What trogon actually is

Trogon runs **fleets of autonomous agents in production**. They aren't summoned one at a
time in a chat window — they're triggered by the outside world: a GitHub webhook, a Slack
message, a Linear ticket, an incident.io page, a cron schedule (`trogon-gateway`,
`trogon-scheduler`, the source crates). Many run at once, for different tenants,
unattended, and each one **takes real actions with real side effects** — opening pull
requests (`trogon-pr-actor`), spending tokens, calling tools and MCP servers, mutating
external systems.

That is the defining fact, and it dictates everything about the front-end. The user of
this UI is not *chatting with* an agent. They are **operating a fleet they cannot watch in
real time and did not personally start.**

### The operator's real problems

From that fact, four concrete problems fall out — each one a question an operator will
ask, and each already half-answered in the codebase:

1. **"What is the fleet doing right now, and is it healthy?"**
   Dozens of asynchronous sessions across tenants. There is no single conversation to
   scroll. The operator needs an at-a-glance, queryable view of live and recent work —
   exactly what `trogon-console`'s session read models already serialize (status, tokens,
   cost, duration, linked agent).

2. **"This agent is about to do something irreversible — should it?"**
   An autonomous agent that can open a PR or post to a customer Slack needs a human gate
   on the dangerous actions. The platform already models this: `trogon-vault-approvals`
   (`proposal` / `notifier` / `stream`) exists specifically to suspend an agent mid-flight
   and wait for a human decision. The UI is the missing half — the place that decision
   gets made.

3. **"It did something surprising three hours ago — why?"**
   With autonomous agents, the hardest operational question is *reconstructing intent
   after the fact*. You cannot debug what you cannot replay. Trogon's back-end is
   **event-sourced** (`trogon-decider`'s `decide`/`evolve`, the `trogonai-proto` event
   aggregates like the scheduler's `created/paused/resumed/removed`, all persisted in
   JetStream). Every state change is already a durable, ordered, causally-linked fact. The
   history of *why* exists — it simply has no surface.

4. **"Did it actually do a good job?"**
   Autonomy without a quality signal is unmanageable. `trogon-outcomes` already encodes
   this — `Rubric`, `Criterion`, `EvaluationResult`, pass/fail against a threshold. The
   operator needs to see those scores against the runs that produced them.

### Why these problems force a specific kind of front-end

Notice what each problem rejects:

- A **chat UI** fails problem 1 — you can't represent a hundred concurrent,
  externally-triggered sessions as a conversation.
- A **CRUD admin panel** fails problems 2 and 3 — it shows the *current* state of a row,
  but autonomous operation is about *decisions over time* and *interventions in flight*,
  neither of which a CRUD form expresses.
- A **log viewer** fails problem 4 and most of 3 — raw logs aren't causal, aren't tied to
  commands, and carry no quality signal.

What all four problems share is that they are about **events and decisions, not records**:
*what did the fleet decide, why, what needs my approval, and how well did it turn out.* The
right front-end is therefore one whose primary objects are the **agent's decisions and
their consequences** — an operator surface organized around the event stream, the
intervention points, and the outcomes. Records (agents, skills, environments) still need
editing, but they are the *supporting cast*, not the center.

This is not a stylistic preference. It is the only shape that answers the four questions
the domain actually generates.

### Why the architecture makes it cheap

The reason to do this now — rather than ship a CRUD panel and retrofit later — is that
**the back-end already did the hard part.** Event sourcing means the audit trail is not a
feature to build; it is a projection to expose. The approvals crate means
human-in-the-loop is not new machinery; it is an endpoint and a queue. The outcomes crate
means quality scoring is a read model away. The expensive, invasive capabilities are
already in `platform`. The front-end's job is to **render decisions the system already
records and resolve interventions the system already raises** — and to keep records
editable along the way.

### How we'll design it

Because the domain is events-and-decisions, we design each screen with **Event Modeling**:
every surface is a swimlane of `wireframe -> command -> event -> view`, paired with a
Given-When-Then specification that doubles as its `trogon-e2e` test. This isn't borrowed
product vision — it's a design *method*, and it's the exact technique for a system whose
source of truth is an event log. It guarantees the discipline this UI needs: every value
on screen traces to an event (its origin), and every action traces to a command (its
destination).

### A note on prior art and honesty

This proposal converges on three central surfaces — intent, audit timeline, approvals —
that also happen to be what 8090's "Software Factory" leads with. That convergence is
real and worth naming openly: any serious tool for *operating autonomous agents in
production* tends to land near intent / timeline / approvals. Two of the three (audit
timeline, approvals) fall directly out of trogon's own event-sourced architecture and the
`trogon-vault-approvals` crate, independent of any external reference. The third (intent /
runs) is the most externally-influenced framing. We adopt the *method* (Event Modeling)
and acknowledge the *convergence* (8090) rather than claiming the shape is wholly original.
An explicit goal for future iterations is to identify the design decisions that come from
trogon being **agent-ops, not SDLC** — the places where this surface should diverge from a
software-factory shape rather than mirror it.

---

## 2. What we're building on (the `platform` branch reality)

| Crate | Role for the front-end |
|---|---|
| `trogon-console` | **The BFF.** Axum HTTP API on port 8090. Already exposes agents, skills, environments, credentials, sessions, mcp-registry. Repository traits backed by NATS KV/JetStream. |
| `trogon-decider` | The command->event engine (`decide`/`evolve`, `WritePrecondition`, multi-step `Act` plans). Defines the *commands* the UI dispatches. |
| `trogonai-proto` | **The shared contract** (buf/protobuf). Fully event-sourced aggregates already exist — e.g. the scheduler with `schedule_created / paused / resumed / removed` events, `state`, `status`, `delivery`. 18 generated read-model `__view` types. |
| `trogon-vault-approvals` | `proposal` / `notifier` / `stream` / `subjects` — the backbone of **approval gates**. |
| `trogon-outcomes` | `Rubric` / `Criterion` / `EvaluationResult` / `EvaluateTrigger` — the **outcomes/scoring** surface. |
| `trogon-gateway` | Translation of external webhooks (GitHub, GitLab, Discord, incident.io…) into domain events — the *event sources* the UI visualizes. |
| NATS JetStream | The event store / KV — the source of every view and the raw material of the audit timeline. |

The console's current router already gives us full CRUD + versioning + rollback for agents,
version history for skills, environment archival, and vault/credential management. The
front-end's job is largely to render these and add the operator surfaces on top.

---

## 3. Design principles

1. **Operator surface, not chatbot.** The center of gravity is intent + timeline +
   approvals, not a text box.
2. **Thin client, fat contract.** No business logic in the browser. Screens render views
   (queries) and dispatch commands (mutations). All types come from `trogonai-proto`
   codegen — zero contract drift.
3. **Every screen is an event-model slice.** Designed as
   `wireframe -> command(s) -> event(s) -> view(s)`, with a Given-When-Then spec that
   doubles as the integration test (`trogon-e2e`).
4. **Audit is the spine.** Any aggregate's event history is a first-class, navigable view
   — not an admin afterthought.
5. **Human-in-the-loop by default.** Sensitive agent actions route through approval gates;
   the UI is where they're resolved.
6. **Tenant-aware everywhere.** Sessions already carry `tenant_id`; the UI treats tenant as
   a top-level scoping primitive.

---

## 4. Information architecture

Eight top-level surfaces, each mapping to existing or near-existing console capabilities:

```
Trogon Console
├── Overview        — fleet health, active sessions, pending approvals, recent events
├── Intent / Runs   — declare an objective -> launch agent -> watch it execute  ★ NEW
├── Sessions        — live + historical agent sessions, token/cost, transcript, timeline
├── Agents          — catalog, definition editor, versions, rollback, per-agent sessions
├── Skills          — catalog + version history
├── Environments    — environments, archival, scoped vault & credentials
├── Capabilities    — MCP registry + tools catalog (the governed catalog)
├── Schedules       — event-sourced scheduler (created/paused/resumed/removed)
├── Outcomes        — rubrics, criteria, evaluation results & scores
└── Approvals       — pending proposals, resolve (approve/reject)  ★ control gate
```

★ = the two surfaces that turn the existing CRUD console into a true operator surface.

---

## 5. The three operator surfaces (the differentiators)

### 5.1 Intent / Runs
The "declare it in plain language first" idea — which falls directly out of problem 1 (you
direct agents you don't hand-start) — made concrete:

- A run starts from an **objective** (free text) + optional structured params (agent,
  environment, tools allowed).
- The system records this as a command; the agent executes; the UI shows progress as an
  **event timeline**, not a wall of chat.
- Each run links to its session, its outcome evaluation, and any approvals it triggered.

This is the marquee screen. It reframes trogon from "a tool that runs agents" to "a place
where you direct agents and stay in control."

### 5.2 Event Timeline (audit)
- A reverse-chronological, filterable stream of domain events for any aggregate (session,
  agent, schedule, run).
- Backed by JetStream — delivered via **SSE/WebSocket** from a new `trogon-console`
  streaming endpoint.
- Each event is expandable (payload, actor, causation/correlation). This *is* the audit
  trail; the answer to "why did it do that?" comes for free.

### 5.3 Approval gates
- A queue of pending `proposal`s from `trogon-vault-approvals`.
- Each shows: what the agent wants to do, why, the blast radius, and Approve / Reject.
- Resolution is a command; the agent unblocks. Notifier integration (Slack/Discord)
  already exists server-side.

---

## 6. Technical architecture

### 6.1 Stack
- **Framework:** React + TypeScript (largest ecosystem, best codegen tooling). SolidJS is a
  viable lighter alternative if bundle size matters more than ecosystem.
- **Data layer:** TanStack Query — views become queries, commands become mutations, with
  cache invalidation driven by incoming events.
- **Routing:** TanStack Router or React Router (file-based).
- **Styling/components:** Tailwind + a headless component lib (Radix / shadcn) for an
  auditable, themeable design system.
- **State:** minimal global state; server is the source of truth. Local state only for
  forms and UI.
- **Real-time:** native EventSource (SSE) for the timeline; upgrade to WebSocket if
  bidirectional needs emerge.

### 6.2 The contract pipeline (critical)
This is what makes the "thin client, fat contract" principle real:

- Today `buf.gen.yaml` generates **Rust only** (`protoc-gen-buffa`).
- **Add a TypeScript plugin** (`protoc-gen-es` / Connect-ES, or `ts-proto`) to
  `buf.gen.yaml`, outputting a `packages/proto-ts` consumed by the front-end.
- Result: commands, events, and read-model `__view` types are **identical** on both sides.
  A back-end schema change breaks the front-end build — exactly what we want.

### 6.3 Transport — phased
- **Phase 1:** REST against `trogon-console` (it's already Axum, already there). Fastest
  path to a working UI.
- **Phase 2:** Add **Connect** (gRPC-web/Connect protocol) for the streaming surfaces
  (timeline, live run progress), since the proto contract already exists. Connect-ES on the
  client pairs naturally with the proto codegen.
- The BFF stays `trogon-console`; we extend its router rather than introduce a new service.

### 6.4 BFF extensions needed
Concrete endpoints to add to `trogon-console` (the rest already exist):

```
GET  /runs                         list runs
POST /runs                         start a run (objective + params)   [command]
GET  /runs/{id}                    run detail + linked session/outcome
GET  /events/{aggregate}/{id}      event history (audit) for an aggregate
GET  /events/stream                SSE feed (filter by tenant/aggregate)
GET  /approvals                    pending proposals
POST /approvals/{id}/resolve       approve|reject                      [command]
GET  /schedules                    scheduler read models
POST /schedules ... pause/resume   scheduler commands
GET  /outcomes / /outcomes/{id}    rubrics + evaluation results
```

---

## 7. Domain-by-domain screen breakdown

Each domain lists the **views** (what's shown), the **commands** (what's dispatched), and
where it already exists in the console router.

### Agents *(exists: full CRUD + versions + rollback)*
- **Views:** agent list (status, model, version), agent detail (`AgentDefinition`: system
  prompt, skills, tools, mcp_servers, metadata), version history, per-agent sessions.
- **Commands:** create, update, delete, rollback-to-version.
- **UX:** a definition editor with a diff view between versions; "rollback" is a guarded
  action with a confirmation that shows the diff.

### Sessions *(exists: read-only list + detail)*
- **Views:** `ConsoleSession` — status (Running/Idle), message_count, input/output/cache
  tokens, cost, duration, linked agent. Plus full transcript.
- **Commands:** none today (read model). Future: stop/resume.
- **UX:** live token & cost meter, transcript viewer, and a per-session **event timeline**
  tab.

### Skills *(exists: CRUD + version history)*
- **Views:** skill catalog, version list.
- **Commands:** create, create-version, delete.

### Environments & Credentials *(exists: CRUD + archive + vault)*
- **Views:** environments (with archived state), per-environment vault + credentials list.
- **Commands:** create/update/delete/archive environment; create/delete credential.
- **UX:** credentials are write-only/masked; surface *where* secrets are used (ties to the
  gateway/vault model). Never display secret values.

### Capabilities (MCP registry + tools) *(partially exists: list)*
- **Views:** registered MCP servers, available tools — the governed catalog of what agents
  can reach.
- **UX:** searchable catalog; show which agents consume each capability.

### Schedules *(proto aggregate exists; needs console routes + UI)*
- **Views:** schedule list with status, derived from
  `schedule_created/paused/resumed/removed` events; delivery config.
- **Commands:** create, pause, resume, remove.
- **UX:** This is the **showcase event-model screen** — its lifecycle is literally a
  sequence of events, so the timeline and the state view are one and the same. Great demo
  of the methodology.

### Outcomes *(crate exists; needs console routes + UI)*
- **Views:** rubrics (`Rubric` + `Criterion`), `EvaluationResult` with `CriterionScore`s
  and pass/fail against `passing_score`.
- **UX:** per-run scorecards; aggregate quality dashboards over time.

### Approvals — see §5.3.

---

## 8. The Event Modeling design system (how we actually design each screen)

For every slice, produce one artifact (Excalidraw/Miro or a markdown table) before writing
code:

```
Swimlane: <actor>
┌─ Wireframe ──────────────┐
│  [screen sketch]         │
└──────────┬───────────────┘
   command ▼            ▲ view
   ┌─────────────┐   ┌─────────────┐
   │ StartRun    │   │ RunTimeline │
   └──────┬──────┘   └──────▲──────┘
          ▼ event           │ projection
        RunStarted ─────────┘
```

Plus a Given-When-Then spec, e.g.:

> **Given** an active agent and a non-archived environment, **When** I start a run with
> objective X, **Then** a `RunStarted` event is recorded and the Runs view shows it as
> in-progress.

That spec is simultaneously: the design doc, the acceptance criteria, and the
`trogon-e2e` test. This is how we guarantee "every field has an origin and a destination."

---

## 9. Cross-cutting concerns

- **Multi-tenancy:** tenant is a top-level scope (sessions already carry `tenant_id`).
  Tenant switcher in the shell; all queries tenant-scoped; the event stream filters by
  tenant.
- **Auth:** front-end assumes an authenticating reverse proxy / token in front of
  `trogon-console`; surface the current actor for the audit trail. (Coordinate with how the
  gateway/console authenticates today.)
- **Errors:** typed errors from `trogon-console::error` surfaced as actionable UI states,
  never raw 500s.
- **Empty/loading/error states:** designed per screen (operator surfaces live or die on
  these).
- **Observability:** front-end emits its own telemetry; `trogon-telemetry`/`trogon-datadog`
  already exist server-side.
- **Accessibility & theming:** dark/light, keyboard-navigable (operators live in this
  tool).

---

## 10. Roadmap

**Phase 0 — Foundations (1–2 wks)**
- Add TS plugin to `buf.gen.yaml`; publish `proto-ts` package.
- Scaffold app (React+TS, TanStack Query/Router, Tailwind, design tokens).
- Generated REST client against `trogon-console`; app shell + tenant switcher + auth
  wiring.

**Phase 1 — Read-only console (2–3 wks)**
- Agents, Sessions, Skills, Environments, Credentials, Capabilities — all backed by
  existing routes.
- Ship value immediately: a real window into the running fleet.

**Phase 2 — The operator surface (3–4 wks)**
- Add console routes for events/stream, runs, approvals.
- Build Intent/Runs, the Event Timeline (SSE), and the Approvals queue.
- This is where trogon stops being a CRUD console and becomes something you operate a live
  fleet from.

**Phase 3 — Authoring & lifecycle (2–3 wks)**
- Agent definition editor + version diff + rollback; skill versioning; environment
  lifecycle.

**Phase 4 — Schedules & Outcomes (2–3 wks)**
- Scheduler UI (the flagship event-model screen) + Outcomes scorecards/dashboards.
- Migrate streaming surfaces to Connect.

**Phase 5 — Polish & scale**
- Perf (virtualized lists for long timelines), a11y, theming, error taxonomy, demo data.

---

## 11. Open decisions (need your call)

1. **Framework:** React (ecosystem/codegen maturity) vs. SolidJS (lighter). Default: React.
2. **Transport for v1:** REST-first then Connect, or commit to Connect/gRPC-web from day
   one?
3. **Repo layout:** front-end inside the monorepo (`web/` or a `packages/` workspace next
   to `rsworkspace/`) vs. a separate repo. Monorepo keeps the proto contract atomic —
   recommended.
4. **Auth model:** what sits in front of `trogon-console` today (proxy? token? tenant
   claims)? This shapes the shell.
5. **Scope of v1:** ship read-only console first (fast win), or hold for the full operator
   surface?

---

## 12. Definition of done (per slice)

A slice is done when: it has an event-model diagram + GWT spec; the generated types are
used end-to-end; loading/empty/error states exist; a `trogon-e2e` test asserts the GWT;
and (for operator-surface slices) the action appears in the event timeline.
