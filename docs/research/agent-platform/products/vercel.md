# Vercel (AI SDK + platform): what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence from ai-sdk.dev, vercel.com docs/blog, and the `vercel/ai` source.
Vercel has *three* agent stories: the AI SDK code object, the platform
primitives (Workflows/Sandbox/Gateway), and **eve**, a filesystem-first
agent framework ("Like Next.js for web apps, but for agents").

## The `agent` noun (primary-source quotes)

- Canonical AI SDK definition: "Agents are **large language models (LLMs)**
  that use **tools** in a **loop** to accomplish tasks." The loop's two jobs:
  "Context management: maintaining conversation history and deciding what
  the model sees at each step" and "Stopping conditions: determining when
  the loop (task) is complete." (ai-sdk.dev/docs/agents/overview)
- In AI SDK 6, "**`Agent` is an interface rather than a class**"
  (`version: 'agent-v1'`, optional `id`, `tools`, `generate()`, `stream()`),
  anything implementing generate/stream over steps *is* an agent.
  `ToolLoopAgent` is the stock implementation: "runs tools in a loop... The
  loop continues until: a finish reason other than tool-calls..., a tool
  without an execute function..., a tool call needs approval..., or a stop
  condition is met (default `isStepCount(20)`)." (`tool-loop-agent.ts`)
- AI SDK 5 was explicit that the class adds nothing: "It doesn't add new
  capabilities, everything you can do with `Agent` can be done with
  `generateText` or `streamText`. Instead, it allows you to encapsulate your
  agent configuration and execution."
- `HarnessAgent` wraps established harnesses ("Claude Code, Codex, or Pi")
  behind the same interface: harness-as-agent.
- **eve** re-introduces the file-based definition: "a filesystem-first
  framework for durable backend AI agents. You define each agent with files
  under an `agent/` directory" (`instructions.md`, `agent.ts`, `tools/*`,
  `skills/*`, `subagents/*`, `channels/*`, `connections/*`, `sandbox/*`).
- Separately, **Vercel Agent** (dashboard product) is a Vercel-operated
  agent for your projects: agent-as-product, priced per investigation.
- **Conceptual model: agent-as-loop (interface over code)**, with eve
  layering agent-as-filesystem-config and Workflows adding durability.

## Subagents

- First-class docs page: "A subagent is an agent that a parent agent can
  invoke. The parent delegates work via a tool, and the subagent executes
  autonomously before returning a result."
- **Isolation:** "Each subagent invocation starts with a fresh context
  window"; nothing inherited by default ("you can manually pass conversation
  history if needed, though this defeats some isolation advantages").
- **The distinctive knob is `toModelOutput`:** "Users see full subagent
  execution (tool calls, intermediate steps), while the model receives only
  a condensed summary. A 100,000-token exploration becomes a focused
  summary." The human-view/model-view split is explicit API.
- Constraint: "Subagent tools cannot use approval flows."
- eve's version distinguishes skill vs subagent cleanly: "Unlike a skill,
  which adds instructions to the running agent, a subagent runs as a
  separate agent with fresh conversation history and state. eve offers two
  kinds: the built-in `agent` tool delegates to a copy of the current agent,
  and declared subagents live under `agent/subagents/*` with their own
  config."

## Configuration surface (what, where, why)

- **`ToolLoopAgentSettings`:** `model` (required), `instructions` (the
  field is `instructions`, not `system`), `tools` + `toolChoice` +
  `activeTools` + `toolOrder`, `stopWhen` (default `isStepCount(20)`),
  `output` (structured output), `runtimeContext` and per-tool
  `toolsContext`, `toolApproval` / tool-level `needsApproval`
  (human-in-the-loop "with a single flag"), `prepareStep`, `prepareCall`
  (per-call templates via `callOptionsSchema`), repair/refine hooks,
  telemetry, lifecycle callbacks (`onStart`... `onEnd`).
- **Loop-control primitives:** `stopWhen` conditions (`isStepCount`,
  `hasToolCall`, `isLoopFinished`, custom predicates over `steps`); the
  forced-completion pattern ("Combine `toolChoice: 'required'` with a `done`
  tool lacking an execute function"); default 20-step cap as "a safety
  measure to prevent runaway loops that could result in excessive API
  calls."
- **Context knobs with stated rationale:** "Agents often need server-side
  state that should not be placed directly in the prompt, such as tenant
  settings, request IDs, feature flags, credentials... Use `runtimeContext`
  as the agent's shared runtime state... Use `toolsContext` for per-tool
  values such as API keys or scoped permissions; each tool receives only its
  own typed context."
- **Where config lives:** pure TypeScript for the SDK; `agent/` directory
  files for eve; `wrangler`-less, platform wiring is implicit (AI Gateway
  model strings like `'anthropic/claude-sonnet-4.5'` route without SDK
  changes).

## Binding time

- **Everything binds in code at construction**, with two dynamism layers:
  `prepareCall` (once per invocation) and `prepareStep` (before every step,
  can swap `model`, `activeTools`, `instructions`, `messages`, contexts, and
  even the sandbox; "if you return a `messages` override, those messages
  carry forward to later steps"). Per-step rebinding is the SDK's signature
  feature: mid-*run* mutation as a designed capability, unique in this
  study.
- **No agent versioning** (git is the versioning); `version: 'agent-v1'` is
  an interface spec tag.
- **Workflows change the binding story for durability:** "'use workflow'
  marks a function as durable... All inputs and outputs are recorded in an
  event log. If a deploy or crash happens, the system replays execution
  deterministically." Steps ("'use step'") compile into isolated API routes
  with retries. **Skew protection pins runs to their deployment:** "Workflows
  keep running on the deployment they were created on, so you can deploy new
  versions... without affecting existing runs", the code-level equivalent
  of OpenComputer's revision pinning.

## Relationships between nouns

- **Agent 1:N invocations; invocation 1:N steps** (step = one LLM call +
  tool executions); `StopCondition` sees the accumulated `steps`.
- **No session noun in the SDK**: messages are the caller's problem. eve
  adds it: "A session is the durable conversation or task started by a
  channel or HTTP request. Each user message or external event creates a
  turn," with sessions running on Workflows.
- **Platform composition:** Queues ("the lower-level primitive that powers
  Vercel Workflows") → Workflow (durable event log) → agent invocation →
  Sandbox (Firecracker microVM, snapshot-on-stop, 5-min default timeout,
  passed as `experimental_sandbox`) and AI Gateway (model routing/failover).
- **As Managed Agents sandbox provider:** "Anthropic provides the model,
  harness, tools, and session state. Self-hosting lets you bring the
  execution environment", one Sandbox microVM per session, Workflow as the
  durable session record, and OIDC credential brokering ("The environment
  key never enters the VM").
- Marketing stack (vercel.com/agents): AI SDK, AI Gateway, Sandbox,
  Workflows, Passport, eve; homepage headline "Agentic Infrastructure:
  build agents on infrastructure that thinks like them."

## Lifecycle

- **The SDK agent has no lifecycle**: plain object; each
  `generate()`/`stream()` call is independent; loop ends on stopWhen /
  non-tool finish / unexecutable tool / approval needed.
- **Durability is opt-in by composition:** Workflow runs sleep for free
  (`await sleep('7 days')`), pause on hooks (`defineHook` /
  `hook.resume(token, data)`), survive deploys; Sandbox snapshots its
  filesystem on stop and auto-resumes.
- **Who runs the loop: customer code** on Vercel Functions (Fluid Compute)
  or anywhere Node runs. Nothing persists by default; persistence comes from
  Workflow event logs, Sandbox snapshots/Drives, or external storage; eve
  surfaces "Agent Runs" observability in the dashboard.
- Their production lesson (blog): the sweet spot is "low cognitive load and
  high repetition from humans," with human review at critical decision
  points.

## What makes it "an agent" here (our inference)

Our inference: Vercel defines the agent as **the loop itself**, "an LLM
that uses tools in a loop", and ships it as an interface so that a
config-object (`ToolLoopAgent`), a wrapped harness (`HarnessAgent`), or
anything else can stand behind the same `generate`/`stream` contract.
Identity, sessions, and durability are deliberately *not* the agent's
problem: they come from composing the loop with platform primitives
(Workflows for durable state, Sandbox for execution, Gateway for models),
and eve is Vercel packaging exactly that composition into a filesystem
convention. For a service design, the two standout ideas are per-step
rebinding (`prepareStep`) and the human-view/model-view split
(`toModelOutput`).

## Open questions

- eve is early; its session/turn data model and versioning story are thin in
  the docs read.
- `experimental_sandbox` threading through prepareStep suggests
  sandbox-per-step swapping; semantics undocumented.
- Vercel Agent (the dashboard product) and Passport were only surface-read.
- x402 (agent payments) and BotID surfaced only via search; not yet core to
  their agent docs.
