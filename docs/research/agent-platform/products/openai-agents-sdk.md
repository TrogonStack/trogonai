# OpenAI Agents SDK + AgentKit: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence from the Agents SDK docs, the `openai-agents-python` source, and the
Assistants API migration guide. AgentKit/Agent Builder pages were not
publicly reachable (403); noted under Open questions.

## The `agent` noun (primary-source quotes)

- SDK docs: an agent is "a large language model (LLM) configured with
  instructions, tools, and optional runtime behavior such as handoffs,
  guardrails, and structured outputs."
- Class docstring (`src/agents/agent.py`): "An agent is an AI model
  configured with instructions, tools, guardrails, handoffs and more."
- It is a **plain Python `@dataclass`**, no server-side ID, no CRUD, no
  persistence. Fields: `name` (required), `instructions` (str **or a
  callable** evaluated per run), `prompt` (server-side Prompt object),
  `handoffs`, `model`, `model_settings`, `tools`, `mcp_servers` ("You are
  expected to manage the lifecycle of these servers"), `input_guardrails`,
  `output_guardrails`, `output_type`, `hooks`, `tool_use_behavior`,
  `reset_tool_choice` ("ensures that the agent doesn't enter an infinite
  loop of tool usage").
- Platform guide framing: "Agents are applications that plan, call tools,
  collaborate across specialists, and keep enough state to complete
  multi-step work." Practical Guide PDF: "systems that independently
  accomplish tasks on your behalf."
- **History, the deprecated Assistants API is the contrast case:**
  "Assistants were persistent API objects that bundled model choice,
  instructions, and tool declarations, created and managed entirely through
  the API" (id, CRUD, Threads, polled Runs). It shuts down August 26, 2026;
  the migration maps Assistants → Prompts (server-side versioned config),
  Threads → Conversations, Runs → Responses. Stated rationale: Responses API
  offers "better performance and new features"; "Your application code now
  handles orchestration... while your prompt focuses on high-level behavior."
- **Conceptual model: agent-as-code-object.** OpenAI moved *away* from
  agent-as-API-resource (Assistants) toward ephemeral code objects, while
  quietly re-introducing server-side versioned config through the `prompt`
  field ("Prompts allow you to dynamically configure the instructions, tools
  and other config for an agent outside of your code").

## Subagents

Two delegation mechanisms, distinguished precisely by the SDK itself:

- **Handoffs**, control transfer. "Handoffs are sub-agents that the agent
  can delegate to" (the `handoffs` constructor field). They surface to the
  LLM as tools named `transfer_to_<agent_name>`. "It's as though the new
  agent takes over the conversation, and gets to see the entire previous
  conversation history", customizable via `input_filter` ("By default, the
  new agent sees the entire conversation history").
- **Agent-as-tool**, result return. From `Agent.as_tool()`: "In handoffs,
  the new agent receives the conversation history... and takes over the
  conversation. In this tool, the new agent receives generated input... and
  the conversation is continued by the original agent."
- Declaration is **static in the constructor** (the roster is the `handoffs`
  list), with dynamic enable/disable via `is_enabled` callables. "Handoffs
  stay within a single run." No depth limit documented.
- Nothing is inherited: each target agent brings its own instructions,
  model, and tools. What *is* shared run-wide is the context object: "every
  agent, tool function, lifecycle etc for a given agent run must use the
  same type of context."
- Guardrails scope by chain position: input guardrails run "only if the
  agent is the first agent in the chain"; output guardrails "only if the
  agent produces a final output."

## Configuration surface (what, where, why)

- **Agent constructor:** full field table above; notable design choices are
  dynamic `instructions` (a function of run context), `output_type`
  (structured output = loop termination condition), and
  `tool_use_behavior` (`run_llm_again` default vs `stop_on_first_tool` /
  custom).
- **RunConfig (per-run):** `model` ("will override the model set on every
  agent"), `model_settings`, global `handoff_input_filter`, run-level
  guardrails, tracing fields (`workflow_name`, `trace_id`, `group_id`),
  `call_model_input_filter` ("allows editing input sent to the model e.g. to
  stay within a token limit"), session settings, sandbox and tool-execution
  configs. `DEFAULT_MAX_TURNS = 10`.
- **Where config lives:** pure code, except the optional `prompt` field
  pulling from a server-managed, versioned Prompt resource ("snapshot,
  review, diff, and roll back"), the bridge back to server-side config.
- **Stated rationale:** handoffs, "Allows for separation of concerns and
  modularity"; guardrails, "you can run a guardrail with a fast/cheap
  model... saving you time and money."

## Binding time

- Everything binds at **construction/run time in code**. Dynamic
  instructions re-evaluate per invocation; `agent.clone()` is
  `dataclasses.replace` (shallow copy); RunConfig overrides apply per run.
- **No versioning of agents**, except via the server-side Prompt resource
  and, historically, Assistants CRUD. The industry-historical note: OpenAI
  had versioned server-side agents (Assistants), deprecated them, and now
  versions only the *prompt config*, not the agent.

## Relationships between nouns

- **Agent : Run, many-to-many.** `Runner.run(starting_agent, input)` is a
  stateless classmethod; one agent can start many runs, and one run can
  traverse many agents via handoffs. The loop (from `run.py`): invoke agent
  → if `output_type`-matching final output, stop → if handoff, loop with new
  agent → else run tools and loop. "The rule for whether the LLM output is
  considered as a 'final output' is that it produces text output with the
  desired type, and there are no tool calls."
- **Session**, conversation state, independent of any agent: "Session
  stores conversation history for a specific session, allowing agents to
  maintain context without requiring explicit manual memory management."
  Implementations: SQLite (memory/file), Redis, OpenAIConversationsSession
  (server-side). Keyed by `session_id`; passed to `Runner.run(session=...)`.
- **Trace/Span**, "Traces represent a single end-to-end operation of a
  'workflow'"; agent, generation, function, guardrail, and handoff spans.
- **Handoff**, a frozen dataclass wrapping the target agent as a tool
  (`tool_name`, `input_filter`, `is_enabled`).
- **Thread**, legacy Assistants noun, mapped to Conversations.
- A **turn** is "one AI invocation (including any tool calls that might
  occur)."

## Lifecycle

- **The agent has no lifecycle**, no create/start/stop/delete; it is
  constructed and garbage-collected. Only sessions and traces persist.
- The run loop enforces `max_turns` (`MaxTurnsExceeded`), supports
  interruption/approval via a resumable `RunState`, and streams via
  `run_streamed`.
- **The customer's process owns the loop entirely** (library model), with
  the Responses API as the server-side primitive ("agentic by default,"
  `previous_response_id` chaining, `store: true` for server-kept state).
- Assistants lifecycle (create/retrieve/modify/delete + polled runs) is the
  deprecated contrast.

## What makes it "an agent" here (our inference)

Our inference: for OpenAI, an agent is **a configured invocation pattern,
not an entity**, a bundle of instructions + tools + termination rule
(`output_type`) that a stateless runner loops over until final output. The
noteworthy datum for a service design is directional: OpenAI *had* the
server-side agent resource everyone else is building (Assistants), and
retired it in favor of code objects + a versioned server-side Prompt +
server-side Conversations, decomposing "agent" into config, state, and loop
rather than keeping it one resource.

## Open questions

- AgentKit / Agent Builder / ChatKit docs were unreachable (403); the visual
  workflow builder's noun model (published workflow versions?) is uncaptured.
- The `sandbox` and `tool_execution` RunConfig fields suggest an execution
  environment story that the docs read did not elaborate.
- `nest_handoff_history` is "opt-in beta", handoff history semantics are
  still moving.
- Practical Guide PDF quotes came via search snippets, not direct text
  extraction.
