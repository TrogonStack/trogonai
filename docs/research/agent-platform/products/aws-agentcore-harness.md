# AWS AgentCore Harness: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Follow-up dossier to [Bedrock AgentCore](./bedrock-agentcore.md), which studies
the platform whose Runtime leaves the loop to the customer. This dossier
studies the managed harness, the AgentCore surface where AWS owns the loop.
Evidence snapshot retrieved 2026-07-15. AWS documentation is mutable, so the
sections below name the page and section used:

- AWS Developer Guide,
  [AgentCore harness](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/harness.html),
  [Harness vs. Runtime](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/harness-vs-runtime.html),
  and the harness sub-pages for
  [getting started](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/harness-get-started.html),
  [models](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/harness-models.html),
  [tools](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/harness-tools.html),
  [skills](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/harness-skills.html),
  [memory](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/harness-memory.html),
  [environment](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/harness-environment.html),
  [operations](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/harness-operations.html),
  [versioning](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/harness-versioning.html),
  [export](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/harness-export.html),
  [security](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/harness-security.html),
  and [quotas](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/bedrock-agentcore-limits.html).
- AWS control-plane API,
  [`CreateHarness`](https://docs.aws.amazon.com/bedrock-agentcore-control/latest/APIReference/API_CreateHarness.html)
  and [`UpdateHarness`](https://docs.aws.amazon.com/bedrock-agentcore-control/latest/APIReference/API_UpdateHarness.html);
  data-plane API,
  [`InvokeHarness`](https://docs.aws.amazon.com/bedrock-agentcore/latest/APIReference/API_InvokeHarness.html).
- GA announcement, AWS What's New, posted Jun 17, 2026,
  [AgentCore harness generally available](https://aws.amazon.com/about-aws/whats-new/2026/06/amazon-bedrock-agentcore-harness-generally-available/),
  the companion [AWS ML blog post, 18 JUN 2026](https://aws.amazon.com/blogs/machine-learning/amazon-bedrock-agentcore-harness-is-now-generally-available-go-from-idea-to-production-grade-agent-in-minutes/),
  and the devguide [release notes](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/release-notes.html)
  (public preview April 2026, GA June 2026).
- Official samples,
  [awslabs/agentcore-samples `01-features/01-harness`](https://github.com/awslabs/agentcore-samples/tree/main/01-features/01-harness),
  read at commit `790838a` (main branch, 2026-07-14).
- The loop engine, Strands Agents:
  [agent loop](https://strandsagents.com/docs/user-guide/concepts/agents/agent-loop/)
  and [retry strategies](https://strandsagents.com/docs/user-guide/concepts/agents/retry-strategies/)
  docs, the [strands-agents GitHub organization](https://github.com/strands-agents),
  and the [AWS Open Source Blog launch post, 2025-05-16](https://aws.amazon.com/blogs/opensource/introducing-strands-agents-an-open-source-ai-agents-sdk/).
- Secondary but AWS-authored: "What is an Agent Harness? A hands-on guide
  with AgentCore harness" on
  [AWS Builder Center](https://builder.aws.com/content/3D3890U2niEPp5gxlssoI9HVvWm/what-is-an-agent-harness-a-hands-on-guide-with-agentcore-harness);
  builder.aws.com renders client-side, so quotes were verified against the
  [dev.to cross-post under the AWS organization account](https://dev.to/aws/what-is-an-agent-harness-a-hands-on-guide-with-agentcore-harness-1h33).

## The `agent` noun (primary-source quotes)

- AWS's definition of the harness concept itself: "Every agent has an
  orchestration layer: a loop that calls the model, picks tools, passes
  results back, manages context, and handles failures. Running it in
  production takes real infrastructure underneath: compute, a sandbox,
  secure tool connections, filesystem, memory, identity, and observability.
  Together, they form the **agent harness**, the system that lets an agent
  actually run." (harness)
- The managed version: "The managed agent harness in AgentCore turns that
  work into configuration. You declare what your agent does (model, tools,
  skills, instructions); AgentCore handles the environment, compute, memory,
  identity, networking, and observability that turn the config into a
  running agent." (harness)
- The decisive loop-ownership split (harness-vs-runtime): "**AgentCore
  harness** is a *managed agent harness*, the orchestration loop itself is
  provided, powered by Strands Agents. You declare what the agent is (model,
  system prompt, tools, memory, limits) as configuration, and AgentCore runs
  the loop." Runtime is the inverse: "The orchestration loop is yours."
- The loop engine is named: "The harness is powered by
  [Strands Agents](https://strandsagents.com/), the open-source agent
  framework from AWS." (harness) Strands Agents is AWS-authored (announced
  on the AWS Open Source Blog, built from "production systems inside
  Amazon", used by Amazon Q Developer, AWS Glue, and VPC Reachability
  Analyzer) but lives under its own `strands-agents` GitHub organization,
  not `awslabs`.
- What that loop is, in the engine's own words: "The agent loop operates on
  a simple principle: invoke the model, check if it wants to use a tool,
  execute the tool if so, then invoke the model again with the result.
  Repeat until the model produces a final response." (strandsagents.com,
  Agent Loop) The same while-loop every harness implements; here the author
  and operator is AWS.
- The AWS-official hands-on guide is more explicit about assembly (secondary
  source, AWS organization account on dev.to): "Under the hood, AgentCore
  harness takes your config and assembles a fully wired Strands Agents
  agent: the orchestration loop, tool execution, memory management, context
  handling, streaming, and error recovery are all handled." And: "The
  simplest way I've seen it put is Agent = Model + Harness."
- The harness is a persistent, ARN'd, versioned resource:
  `arn:...:bedrock-agentcore:region:account:harness/Name-XyZ123`, created by
  `CreateHarness`, invoked by `InvokeHarness` (both names literal, confirmed
  against the API references), auto-versioned on every update. The quotas
  page states the layering plainly: "AgentCore harness is a logical
  resource. Each harness you create is backed by a managed AgentCore Runtime
  that AgentCore provisions and operates on your behalf."
- **Conceptual model: agent-as-config over a vendor-owned loop.** The exact
  counterpoint to the Runtime dossier's agent-as-deployed-service: the
  orchestration loop is configured rather than shipped as customer code, so
  the customer artifact is a versioned configuration record, and the loop
  that executes it is authored, patched, and evolved by AWS as a service.
  Customer code can still run at the edges (inline-function tool
  implementations execute client-side, as below), but the loop itself is not
  customer-owned. The customer steers it only through configuration, or
  leaves it entirely via export-to-code.

## Subagents

- **The harness has no subagent noun.** No child agents, no delegation
  primitive, no fan-out configuration appears anywhere in the harness
  config surface or API.
- Multi-agent coordination is explicitly a *graduation trigger out of* the
  harness: "When you outgrow configuration and need custom orchestration
  logic, specialized routing, or multi-agent coordination, you can export
  the harness to Strands code and keep running on the same platform."
  (AWS hands-on guide)
- Agent-to-agent shows up only at the edges: `actorId` "identifies the
  entity interacting with the agent (a user, another agent, or a system)",
  scoping memory per actor (harness-memory); and composition happens above
  the harness, "You can drop a harness into a larger pipeline through the
  AgentCore InvokeHarness state in AWS Step Functions" (harness). Peer
  harnesses can also share a BYO memory resource, as in the Runtime dossier.
- The Agents Classic migration guide names the sanctioned shape: "The
  managed harness supports an agent-as-tool pattern for simpler multi-agent
  use cases" (agents-classic-maintenance-mode), i.e. another agent fronted
  by a Gateway target and listed as one of this harness's tools, a peer
  wiring rather than a parent/child record.

## Configuration surface (what, where, why)

`CreateHarness` requires only `harnessName` and `executionRoleArn` ("The ARN
of the IAM role that the harness assumes when running"). Everything else is
optional with managed defaults:

- **Model:** a `HarnessModelConfiguration` union (`bedrockModelConfig` |
  `openAiModelConfig` | `geminiModelConfig` | `liteLlmModelConfig`). "If you
  don't specify a model, the harness defaults to Anthropic's Claude Sonnet
  4.6 on Amazon Bedrock (`global.anthropic.claude-sonnet-4-6`)." Providers
  can change mid-session: "Switch providers between turns of the same
  session and the conversation continues. Context carries over."
  Third-party keys live in AgentCore Identity's token vault: "The harness
  pulls the key at invocation time. Your agent code never sees raw
  credentials."
- **System prompt:** `systemPrompt` content blocks (the CLI workflow keeps
  it in a `system-prompt.md` file).
- **Tools:** "Tools are declarative. You list what the agent can call;
  AgentCore handles invocation, credentials, and results." Five types
  (`remote_mcp`, `agentcore_gateway`, `agentcore_browser`,
  `agentcore_code_interpreter`, `inline_function`) plus built-ins: "Default
  tools `shell` and `file_operations` are available in every session unless
  you restrict them with `allowedTools`", which takes glob patterns ("`*`
  for all tools, `@builtin` for all built-in tools, or
  `@serverName/toolName` for specific MCP server tools"). Inline functions
  execute client-side: "The harness pauses when the tool is called and
  returns the call to your code", the human-in-the-loop pattern.
- **Skills:** bundles per the AgentSkills.io spec (`SKILL.md` with YAML
  frontmatter plus optional `scripts/`, `references/`, `assets/`), sourced
  from `awsSkills` | `git` | `s3` | `path`. "Skills use progressive
  disclosure: metadata is injected into the system prompt upfront (~100
  tokens), and full instructions are loaded on demand via a tool call."
- **Memory:** `managedMemoryConfiguration` | `agentCoreMemoryConfiguration`
  (BYO) | `disabled`. "By default, the harness provisions an AgentCore
  Memory instance automatically with sensible defaults (semantic +
  summarization strategies, 30-day event expiry)."
- **Context truncation:** `truncation` strategy `sliding_window` (default,
  keep most recent N messages) | `summarization` | `none` ("Use only if you
  manage context size yourself").
- **Execution limits**, with the stated rationale "Set hard caps so a
  runaway agent can't burn through resources": `maxIterations`
  ("reasoning/action cycles per invocation. Default 75."), `timeoutSeconds`
  (default 3600), `maxTokens` (per-invocation output budget, no default),
  `idleRuntimeSessionTimeout` (default 900), `maxLifetime` (default 28800).
- **Environment:** default or custom container (`environmentArtifact`),
  `environmentVariables` (max 50), filesystem mounts (service-managed
  session storage; EFS access point; S3 Files access point, the latter two
  VPC-required), `authorizerConfiguration` (IAM SigV4 default or custom JWT
  authorizer), tags.

Config lives in the control-plane API (`bedrock-agentcore-control`), the
`agentcore` CLI (`create`, `deploy`, `invoke`, `export harness`), or the
console. The stated rationale for the knob design is iteration speed and
cost control: "Trying a different model or adding a new tool is a config
change, not a code rewrite"; "You can test N model/prompt/tool combinations
in the time it would take to redeploy once." The trust boundary is blunt:
"All `InvokeHarness` and `InvokeAgentRuntimeCommand` input is trusted...
The harness does not sanitize input, filter content blocks, or enforce
behavioral constraints."

## Binding time

- The documented principle: "This is the core of the config-based model:
  **defaults at creation time, overrides at invocation time.**"
  (harness-models)
- **At CreateHarness:** all defaults, snapshotted as version 1. "When you
  create a harness, AgentCore automatically creates version 1 (V1)."
- **At UpdateHarness:** "Each update to the harness creates a new version
  with a complete, self-contained configuration"; "Versions are immutable
  once created." `harnessVersion` is "Incremented on every successful
  UpdateHarness." Array fields replace wholesale; optional fields use an
  `optionalValue` wrapper to distinguish leave-unchanged, set, and clear.
- **At InvokeHarness:** per-call overrides for `model`, `systemPrompt`,
  `tools`, `skills`, `allowedTools`, `maxIterations`, `maxTokens`,
  `timeoutSeconds`, `actorId`. "The harness resource stays unchanged; only
  that call uses the overrides." Overrides never mint a version. Invoke-time
  skills "are appended after create-time skills; if both define a skill with
  the same name, the invoke-time version wins."
- **Endpoints are the routing layer**, same design as Runtime: "The
  `DEFAULT` endpoint automatically points to the latest version"; named
  endpoints pin versions and "must be explicitly updated to point to new
  versions", which is also the rollback mechanism ("roll back instantly by
  pointing an endpoint at an earlier version").
- Even the model binds late: mid-session provider switching is a headline
  feature, so the brain is swappable per turn while the loop stays AWS's.

## Relationships between nouns

- **Harness 1:N HarnessVersion** (immutable snapshots) and **1:N
  HarnessEndpoint** (named pointers; DEFAULT auto-tracks latest; quota of 10
  endpoints per agent).
- **Harness 1:1 managed AgentRuntime.** "Each harness you create is backed
  by a managed AgentCore Runtime that AgentCore provisions and operates on
  your behalf", and the relationship is a real schema-level reference: the
  harness environment carries "The ARN of the underlying AgentCore Runtime"
  (`HarnessAgentCoreRuntimeEnvironment.agentRuntimeArn`, required).
  CloudTrail makes the layering visible: "harness resources appear under
  the `AWS::BedrockAgentCore::Runtime` resource type rather than a
  harness-specific type." IAM mirrors it: "calling `InvokeHarness` requires
  both `bedrock-agentcore:InvokeHarness` and
  `bedrock-agentcore:InvokeAgentRuntime` permissions on the harness ARN."
  This declarative wiring is the family's inversion point: the Runtime
  dossier's primitives are wired in customer code with no control-plane FK,
  while the harness references Memory, Gateway, Browser, and Code
  Interpreter by ARN fields in its own schema, and a harness-managed Memory
  points back via `managedByResourceArn`.
- **Harness 1:N Session; Session 1:1 microVM (at a time).** "Every harness
  session is stateful by default and runs in a secure, isolated microVM per
  session (backed by AgentCore runtime)." Sessions are identified by a
  client-supplied `runtimeSessionId` ("must be at least 33 characters...
  Reuse the same session ID across invocations to continue a conversation in
  the same environment"). Isolation is per session, not per invocation; many
  invocations share one microVM until it expires and is replaced.
- **Harness 1:1 managed Memory (by default).** `DeleteHarness` has
  `deleteManagedMemory` ("Default: true. If false, the memory is
  disassociated and becomes a regular customer-owned resource"). Memory
  events are scoped by `actorId` + `sessionId`.
- **Isolation guarantee:** "Every session runs in its own Firecracker
  microVM in AgentCore Runtime. No shared state, no shared filesystem."

## Lifecycle

- **Resource states:** CREATING | CREATE_FAILED | UPDATING | UPDATE_FAILED |
  READY | DELETING | DELETE_FAILED; create, then poll `get-harness` until
  READY.
- **Who owns the loop: AWS.** The loop runs vendor-side per invocation:
  when Memory is enabled (the default), the client sends only the new message
  ("You do not need to pass previous messages yourself") and the harness
  loads history from Memory; when Memory is `disabled`, only the in-microVM
  state carries history, so once the microVM expires (see session
  persistence tiers below) the caller must supply prior conversation context
  itself. Either way the harness runs the Strands loop under the configured
  limits and streams results back.
  `InvokeHarness` "returns a stream of events" (`messageStart`,
  `contentBlockStart`, `contentBlockDelta` carrying text, `toolUse` input,
  and `reasoningContent`, `contentBlockStop`, `messageStop`, `metadata` with
  token usage and latency, `runtimeClientError`). `stopReason` enumerates
  the loop's exit conditions: `end_turn`, `tool_use` (paused for an inline
  function), `max_tokens`, `max_iterations_exceeded`, `timeout_exceeded`,
  `max_output_tokens_exceeded`. The samples describe the unit being capped
  as "think → act → observe loop cycles".
- **What the managed loop owns, precisely.** Confirmed in primary docs:
  context-window management ("When conversation history grows beyond the
  model's context window, the harness applies a truncation strategy"),
  streaming, session-state persistence ("The harness automatically persists
  conversation state in AgentCore Memory... even after the underlying
  microVM session has expired"), and per-session isolation. Error recovery
  is the one commonly-claimed harness duty AWS does *not* document at the
  harness level: the API reference tells the *caller* to "implement
  exponential backoff retry logic in your application" for
  `InternalServerException` and `ThrottlingException`. Automatic retries are
  documented only for the Strands engine itself ("agents retry throttling
  errors up to 5 times (6 total attempts) with exponential backoff starting
  at 4 seconds"), and tool failures feed back into the loop rather than
  killing it ("the error information goes back to the model as an error
  result rather than throwing an exception that terminates the loop").
- **A second door into the environment bypasses the loop.**
  `InvokeAgentRuntimeCommand` "gives you direct shell access to the harness
  microVM: deterministic command execution with no model reasoning, no token
  cost, no ambiguity." Commands run as root; `allowedTools` does not apply,
  only its own IAM action does.
- **Session persistence tiers:** in-microVM state is bounded by
  `idleRuntimeSessionTimeout` (default 15 min) and `maxLifetime` (default
  8 hrs), after which the microVM is replaced; filesystem mounts and Memory
  survive replacement; skills are re-fetched on new sessions "to guarantee
  freshness."
- **Graduation path:** "Export harness to code takes a managed harness
  configuration and generates the equivalent agent as editable Python source
  code using the Strands framework. The generated agent is a normal
  AgentCore runtime agent: you own the code, can modify it freely."
  Framework support "Strands, with other frameworks like Claude Agents SDK
  coming soon." Loop ownership is thus a one-way door by design: config
  until you need code, then you become a Runtime customer.
- **Pricing confirms the harness is pure abstraction:** "There is no
  separate harness charge. You pay only for the underlying AgentCore
  capabilities you use."
- **Timeline:** public preview April 2026; GA announced Jun 17, 2026 (What's
  New) with the ML blog dated 18 JUN 2026. GA added default managed memory,
  more providers via LiteLLM and Bedrock Mantle, the AWS skills catalog,
  evaluations, versioning and endpoints, and export to Strands code.

## What makes it "an agent" here (our inference)

Our inference: in the harness, "agent" is **a versioned configuration record
that a vendor-owned loop makes live**. AWS supplies and operates the loop
code (a fully wired Strands agent assembled from the config); the customer
never sees or ships the loop itself (though inline-function tools can still
execute customer code client-side) and steers it only through declared
fields and per-call overrides. This completes the spectrum inside one product family:
AgentCore Runtime is the strongest agent-as-infrastructure model (the shell
is standardized, the brain is opaque customer code), while the harness is
its inverse (the loop is standardized vendor code, the agent is pure
declaration). It is the same fundamental while-loop found in every harness
in this corpus, differing only in author and degree of control, which is
exactly the axis our
[decision record Q5](../decision-record.md#q5-is-the-agent-bound-to-a-harness-the-providermodel-or-both-harnessagent-vs-agent)
models with the `runtime` field: the AgentCore harness is a live production
example of the `managed-default` pole, with export-to-code as the escape
hatch to the customer-loop pole.

## Open questions

- No harness-level automatic retry or error-recovery policy is documented.
  The devguide's conceptual framing says the harness "handles failures," but
  the API reference delegates backoff to the calling application, and retry
  specifics exist only in Strands SDK docs. Whether the managed loop applies
  the Strands defaults verbatim is unstated.
- The docs are internally inconsistent about one operation name:
  `ListHarnessEndpoints` (harness-versioning) vs `ListHarnessesEndpoints`
  (harness-get-started API list). Unresolved against the service model.
- Behavior of in-flight sessions when the DEFAULT endpoint advances to a new
  version is undocumented, the same gap flagged in the Runtime dossier.
- Whether harness sessions and raw Runtime sessions share the same microVM
  pool and isolation implementation is not spelled out; only the logical
  layering ("backed by a managed AgentCore Runtime") is documented.
- Strands' own monorepo is named `harness-sdk` and its docs navigation
  groups the SDKs under a "Strands Harness" label, but neither side formally
  ties that branding to the AgentCore harness product; treat the shared
  "harness" naming as related but not proven identical.
- No numeric caps on tools, skills, or MCP servers per harness were found;
  the quotas page defers wholesale to Runtime quotas.
- Harness create/get responses carry no `workloadIdentityDetails` field,
  unlike Runtime and Gateway; whether the harness's underlying managed
  AgentRuntime still receives an auto-created workload identity is
  unstated.
- Claude Agent SDK as a second export target is "coming soon" with no
  committed scope or date.
