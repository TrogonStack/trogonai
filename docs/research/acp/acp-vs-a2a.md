# ACP vs A2A: Relationship and Convergence Assessment

Produced 2026-07-30 by a three-angle research pass (technical layering,
governance/convergence, coexistence in practice), each angle adversarially
verified. Two verifier corrections are already applied to the text below.

## TL;DR

Zed's ACP will NOT become A2A. They are orthogonal by design: ACP owns the
client-to-agent seat (editor/host drives a co-located agent, stdio JSON-RPC,
client-mediated fs/terminal/permissions), A2A owns the remote agent-to-agent
seat (network services, AgentCard discovery, durable task lifecycle,
OAuth/mTLS). The "ACP is merging into A2A" story is real but refers to a
DIFFERENT protocol: IBM/BeeAI's Agent Communication Protocol, which merged
into A2A under the Linux Foundation in August 2025. Zed's Agent Client
Protocol remains independently governed (joint lead maintainers from Zed and
JetBrains, RFD process, no foundation donation) and its v2 draft does not
mention A2A at all. The best-documented in-the-wild pattern is gemini-cli:
ACP for the local editor seat, A2A for remote subagent delegation, MCP for
tools, all in one binary. That matches trogonai's existing acp-nats vs
a2a-nats crate boundary (ADR 0034), which this research validates keeping.

# Angle 1: Technical layering

# ACP (Zed) vs A2A (Google): Technical Layering Comparison

Disambiguation first: this compares Zed's Agent Client Protocol (agentclientprotocol.com), not IBM/BeeAI's "Agent Communication Protocol." That other ACP was donated to the Linux Foundation with BeeAI in March 2025, and merged into A2A under LF AI & Data in August 2025, with IBM's Kate Blair joining the A2A Technical Steering Committee ([LFAI & Data](https://lfaidata.foundation/communityblog/2025/08/29/acp-joins-forces-with-a2a-under-the-linux-foundations-lf-ai-data/), [i-am-bee discussion](https://github.com/orgs/i-am-bee/discussions/5)). "ACP is becoming A2A" is true only for the IBM protocol. Zed's ACP is a separate, still-independent spec and is the subject of this note.

## Transport and framing

ACP is JSON-RPC 2.0. Its primary, spec-emphasized transport is a client spawning the agent as a subprocess and speaking JSON-RPC over stdio; the spec also documents HTTP and WebSocket transports for agents running as separate/cloud infrastructure, and explicitly reuses MCP's JSON representations where possible ([agentclientprotocol.com](https://agentclientprotocol.com/overview/introduction)).

A2A supports three protocol bindings that are functionally equivalent: JSON-RPC 2.0 over HTTP, gRPC with Protocol Buffers, and HTTP/REST ([a2a-protocol.org](https://a2a-protocol.org/latest/specification/)). There is no stdio subprocess mode; A2A assumes agents are network-addressable services from the start.

## Session/task model

ACP models a session containing prompt turns: the client calls `session/prompt`, the agent streams `session/update` notifications (message chunks, tool calls, plans, mode changes), and the turn ends when the agent returns a `stopReason` ([prompt-turn](https://agentclientprotocol.com/protocol/v1/prompt-turn)). Verified `StopReason` values: `end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, `cancelled` ([docs.rs schema](https://docs.rs/agent-client-protocol-schema/latest/agent_client_protocol_schema/enum.StopReason.html)). This is a turn-based, conversational model close to a chat completion loop with tool calls in the middle.

A2A models a task with an explicit lifecycle: `SUBMITTED`, `WORKING`, terminal states (`COMPLETED`/`FAILED`/`CANCELED`/`REJECTED`), and interrupt states (`INPUT_REQUIRED`/`AUTH_REQUIRED`). Communication happens via Messages (turns, composed of text/file/data Parts) and Artifacts (task outputs, also composed of Parts) - a deliberate separation of dialogue from deliverable ([a2a-protocol.org](https://a2a-protocol.org/latest/specification/)). Discovery is via the AgentCard, a manifest declaring identity, endpoint, auth requirements, and capabilities, plus AgentSkill entries describing discrete functions with inputs/outputs. Cards can be signed and cached, and "extended" cards can reveal more to authenticated callers.

These overlap where both describe multi-turn exchange with structured content and streamed progress. They diverge where ACP has no first-class notion of a durable, independently pollable task object or a discovery manifest, because it assumes the client already chose and launched a specific agent; A2A has no notion of session-scoped filesystem/terminal delegation, because it assumes the agent is a remote service, not code running against the client's local environment.

## Trust model

ACP's trust model is a co-located subprocess under client control. The client owns the environment: it spawns the agent process, passes credentials as environment variables or headers in MCP server configs, and the agent must connect only to MCP servers the client specifies ([session-setup](https://agentclientprotocol.com/protocol/session-setup); inference: no documented sandboxing or credential-encryption layer in the spec excerpts reviewed, implying reliance on OS-level process trust). Filesystem and terminal access are not ambient; the agent requests permission via `session/request_permission`, a baseline client capability that gates sensitive tool calls before execution ([protocol overview](https://agentclientprotocol.com/protocol/overview)).

A2A's trust model is an opaque remote peer reached over the network. There is no filesystem/terminal delegation concept at all; client and agent negotiate purely through declared capabilities and authenticated API calls. Authorization is enforced per the AgentCard's declared schemes, and servers return `Unauthorized`/`Forbidden` without revealing whether a resource exists ([a2a-protocol.org](https://a2a-protocol.org/latest/specification/)).

## Streaming

ACP streams via `session/update` JSON-RPC notifications over the same stdio/HTTP/WebSocket channel already in use for the session ([protocol overview](https://agentclientprotocol.com/protocol/overview)). A2A streams via Server-Sent Events for `SendStreamingMessage`/`SubscribeToTask`, emitting `TaskStatusUpdateEvent` and `TaskArtifactUpdateEvent`, and separately supports webhook-style push notifications for server-to-server integrations where a persistent connection is impractical ([a2a-protocol.org](https://a2a-protocol.org/latest/specification/)).

## Auth

ACP: environment-variable/header injection at session setup, no dedicated `authenticate` RPC method found in the spec excerpts reviewed; trust is delegated to whoever configured the client ([session-setup](https://agentclientprotocol.com/protocol/session-setup)). A2A: enterprise-grade auth declared per-agent in the AgentCard (API keys, OAuth 2.0, mTLS, etc.), validated server-side per request ([a2a-protocol.org](https://a2a-protocol.org/latest/specification/)).

## Can one agent expose both?

Yes, confirmed in practice: gemini-cli ships an `--acp` "ACP mode" for stdio JSON-RPC control from editors like Zed and JetBrains ([acp-mode.md](https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/acp-mode.md)), and separately ships `@google/gemini-cli-a2a-server` ([npm](https://www.npmjs.com/package/@google/gemini-cli-a2a-server)), an A2A server that can run alongside the CLI on a port for external, network-based control of the same underlying agent. This is the same pattern trogonai already mirrors with its ACP transport over NATS plus its independent a2a-nats crate family (ADR 0034): one agent core, two protocol facades for two different callers.

## Cleanest layering statement

Inference: ACP is the local session protocol between one client and one co-located agent process, owning turn-taking, streaming deltas, and permissioned access to the client's own filesystem/terminal. A2A is the inter-agent network protocol, owning discovery (AgentCard), durable task lifecycle, and authenticated, artifact-producing exchange between agents that do not share a process or a filesystem. In an agent platform, ACP is the boundary an interactive client (editor, IDE, CLI shell) uses to drive an agent it trusts and co-locates with; A2A is the boundary one agent uses to delegate work to another agent it does not trust with local resources and reaches only over the network. For trogonai, that maps to ACP-over-NATS as the human/client-facing edge and a2a-nats as the agent-to-agent delegation fabric, consistent with why both already coexist in the codebase rather than being redundant.

## Sources

- https://agentclientprotocol.com/overview/introduction
- https://agentclientprotocol.com/protocol/overview
- https://agentclientprotocol.com/protocol/session-setup
- https://agentclientprotocol.com/protocol/v1/prompt-turn
- https://docs.rs/agent-client-protocol-schema/latest/agent_client_protocol_schema/enum.StopReason.html
- https://a2a-protocol.org/latest/specification/
- https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/acp-mode.md
- https://www.npmjs.com/package/@google/gemini-cli-a2a-server
- https://lfaidata.foundation/communityblog/2025/08/29/acp-joins-forces-with-a2a-under-the-linux-foundations-lf-ai-data/
- https://github.com/orgs/i-am-bee/discussions/5



# Angle 2: Governance and convergence

## Is Zed's ACP converging with A2A? No, it is orthogonal by design, and the confusion has a specific, traceable root cause

### The direct answer

Zed's Agent Client Protocol (agentclientprotocol.com, github.com/agentclientprotocol) shows no evidence of merging into, being replaced by, or converging with Google's A2A. The two are governed independently, scoped to different layers of the agent stack, and no primary source from Zed, the agentclientprotocol org, Google, or the Linux Foundation states otherwise. The "ACP is becoming A2A" narrative traces to a different, unrelated protocol that happens to share the acronym.

### The root cause of the confusion: IBM/BeeAI's "Agent Communication Protocol"

IBM Research launched its own Agent Communication Protocol (also abbreviated ACP) in March 2025 to power the open-source BeeAI Platform, and donated the BeeAI project, ACP included, to the Linux Foundation that same month (source: LF AI & Data blog, https://lfaidata.foundation/communityblog/2025/08/29/acp-joins-forces-with-a2a-under-the-linux-foundations-lf-ai-data/). Google's A2A launched roughly a month later, in April 2025. On August 25-29, 2025, IBM's ACP officially merged into A2A under Linux Foundation (LF AI & Data) governance. The announcement states independent ACP development is winding down and its team is contributing technology and expertise directly into A2A, with migration paths and documentation provided for existing IBM-ACP users (same source, corroborated by the i-am-bee maintainers' own discussion thread: https://github.com/orgs/i-am-bee/discussions/5). Kate Blair, IBM's Director of Incubation who had overseen ACP, joined the A2A Technical Steering Committee alongside representatives from Google, Microsoft, AWS, Cisco, Salesforce, ServiceNow, and SAP. Blair's quote from the announcement: "By bringing the assets and expertise behind ACP into A2A, we can build a single, more powerful standard for how AI agents communicate and collaborate."

This is a real, well-documented, completed merger. It is just not Zed's protocol.

### Zed's own maintainers have explicitly disambiguated this

In a Zed GitHub Discussions thread asking about "A2A compatibility" (https://github.com/zed-industries/zed/discussions/37519), a community member (chaizhenhua, no verified Zed affiliation per the adversarial check) clarified: Zed's Agent Client Protocol (zed-industries/agent-client-protocol, now hosted under the independent github.com/agentclientprotocol org) is separate from "the Agent Communication Protocol that was merged into A2A under the Linux Foundation." Community members in that thread still expressed interest in future A2A interoperability (e.g., bridging via litellm's A2A mode), but that is a feature request for a bridge or adapter, not evidence of governance convergence.

### Governance homes today

- Zed's ACP: independently governed at agentclientprotocol.com and github.com/agentclientprotocol. Design decisions go through an RFD ("Request for Dialog") process; Ben Brandt (Zed Industries) and Sergey Ignatov (JetBrains) are documented as joint Lead Maintainers in a BDFL-style model, with champions shepherding individual RFDs (https://agentclientprotocol.com/rfds/about). No foundation donation or governance transfer has been announced for Zed's ACP; it remains Apache 2.0-licensed and Zed-led. JetBrains partnered with Zed in October 2025 to co-develop it further, expanding its editor footprint (IntelliJ, PyCharm, WebStorm) rather than changing its governance body.
- A2A: an open source project under the Linux Foundation, originally contributed by Google, now stewarded via a Technical Steering Committee with multi-vendor representation (IBM, Google, Microsoft, AWS, Cisco, Salesforce, ServiceNow, SAP).
- IBM/BeeAI's ACP: effectively absorbed into A2A as of August 2025; no longer independently developed.

### The ACP v2 draft and roadmap signal

The ACP v2 draft announcement (published July 20, 2026, at https://agentclientprotocol.com/announcements/acp-v2-draft) makes no mention of A2A or Agent2Agent anywhere. It reiterates that ACP's core design goal is freedom for "agents and clients," explicitly the client-agent (editor-agent) relationship, not agent-to-agent coordination. There is no RFD or roadmap language positioning ACP as a superset, subset, or future merge target of A2A.

### How the community frames the relationship (and why it holds up technically)

Recurring community and technical commentary (e.g., https://codex.danielvaughan.com/2026/05/01/codex-cli-agent-interoperability-protocols-mcp-acp-a2a/) describes three adjacent, non-competing layers: MCP gives an agent tools, ACP gives an agent a client/editor surface, and A2A gives an agent peer collaborators. A single coding agent can speak all three simultaneously without conflict, which is a structural argument for why convergence is unlikely: they solve different integration problems (tool access, human-facing UI transport, and cross-agent/cross-organization delegation) rather than duplicating one another.

### Assessment: orthogonal-by-design, not merging

The evidence supports "orthogonal by design," not "convergence likely" or "even-odds unlikely." Nothing in Zed's own materials, the agentclientprotocol GitHub org, Google's A2A materials, or Linux Foundation announcements suggests ACP-the-editor-protocol is on any path toward A2A. The one real, sourced "ACP joins A2A" event concerns a different, IBM-originated protocol that no longer exists independently.

**Signals that would change this assessment**: an RFD on agentclientprotocol.com proposing agent-to-agent extensions or a formal donation of Zed's ACP to a foundation; a joint announcement from Zed Industries and the A2A Technical Steering Committee; A2A roadmap documents naming ACP as a client transport it intends to subsume; or the agentclientprotocol GitHub org being folded into a2aproject or the Linux Foundation. None of these have occurred as of this research (July 2026).

### Sources

- [ACP Joins Forces with A2A - LFAI & Data (Aug 29, 2025)](https://lfaidata.foundation/communityblog/2025/08/29/acp-joins-forces-with-a2a-under-the-linux-foundations-lf-ai-data/)
- [ACP Joins Forces with A2A Under the Linux Foundation - i-am-bee Discussion #5](https://github.com/orgs/i-am-bee/discussions/5)
- [A2A compatibility - zed-industries/zed Discussion #37519](https://github.com/zed-industries/zed/discussions/37519)
- [ACP RFDs process - agentclientprotocol.com](https://agentclientprotocol.com/rfds/about)
- [ACP v2 is available in Draft - agentclientprotocol.com](https://agentclientprotocol.com/announcements/acp-v2-draft)
- [Agent Interoperability Protocols and Codex CLI: MCP, ACP, and A2A in Practice](https://codex.danielvaughan.com/2026/05/01/codex-cli-agent-interoperability-protocols-mcp-acp-a2a/)
- [What is Agent Communication Protocol (ACP)? - IBM](https://www.ibm.com/think/topics/agent-communication-protocol)
- [Agent2Agent (A2A) Project - GitHub](https://github.com/a2aproject)

(inference: the "would change this assessment" signals list is analytical judgment, not sourced from any primary document predicting these events.)


# Angle 3: Coexistence in practice

# Coexistence in Practice: ACP and A2A in Real Products

## The pattern: one product, two protocols, two seats

The clearest confirmation of a division of labor comes from a single product: **gemini-cli**. It exposes `--experimental-acp` (`--acp`), which starts Gemini CLI as a subprocess speaking the Agent Client Protocol over stdio/JSON-RPC 2.0 so editors (Zed, JetBrains IDEs via a plugin) can drive it as a local coding agent ([Zed ACP agent page](https://zed.dev/acp/agent/gemini-cli), [gemini-cli acp-mode docs](https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/acp-mode.md), [Guillaume Laforge, IntelliJ integration](https://glaforge.dev/posts/2026/02/01/how-to-integrate-gemini-cli-with-intellij-idea-using-acp/)). Separately, gemini-cli's subagents system lets the *same* CLI delegate tasks to remote, externally hosted agents (for example on Google's Agent Engine) using A2A: a `RemoteAgentInvocation` tool type proxies requests and preserves A2A `context`/`task` IDs across turns, configured via `kind = "remote"` entries in `agents.toml` pointing at an agent-card URL ([gemini-cli subagents docs](https://github.com/google-gemini/gemini-cli/blob/main/docs/core/subagents.md), [PR #16013 adding remote agents](https://github.com/google-gemini/gemini-cli/pull/16013)). So in one codebase: ACP is the local client-to-agent seat, A2A is the remote agent-to-agent seat. Gemini CLI's MCP support (connecting to MCP tool servers) runs alongside both ([geminicli.com ACP mode docs](https://geminicli.com/docs/cli/acp-mode/)), giving a live three-protocol stack in one binary.

**Devin** confirms the same split from the other side: Devin Desktop's Agent Command Center supports running third-party agents via ACP ([Devin docs](https://docs.devin.ai/desktop/acp)); no evidence surfaced of A2A support in Devin itself.

**Goose (Block)** is standardizing on ACP as "the primary interface for all goose clients, desktop, CLI, and beyond," with a dedicated `goose-acp` crate and a phased rollout (Stabilize ACP Server, TypeScript TUI Alpha, Desktop Migration, Consolidation) ([goose discussion #4645](https://github.com/block/goose/discussions/4645), [goose discussion #7309](https://github.com/aaif-goose/goose/discussions/7309)). No direct evidence of Goose speaking A2A.

**Frameworks split along the same seam.** LangGraph/LangSmith's Agent Server exposes an `/a2a/{assistant_id}` endpoint speaking `message/send`, `message/stream`, `tasks/get` ([LangChain A2A docs](https://docs.langchain.com/langsmith/server-a2a)); no ACP support found. CrewAI treats A2A as "a first-class delegation primitive" with both client (`A2AClientConfig`) and server (`A2AServerConfig`) modes ([CrewAI A2A docs](https://docs.crewai.com/en/learn/a2a-agent-delegation)); no ACP support found. Google's own **ADK** is explicitly layered: "ADK to connect to the MCP server, but A2A to expose this functionality to clients" ([Google Developers Blog, agent protocols guide](https://developers.googleblog.com/developers-guide-to-ai-agent-protocols/)). None of LangGraph, CrewAI, or ADK were found to speak ACP; ACP adoption clusters around editor/IDE-facing tools (Zed, JetBrains, Devin Desktop, Goose, gemini-cli, OpenAI's Codex CLI per secondary sources) rather than orchestration frameworks.

## MCP's position

Across every source checked, MCP is consistently the third leg, not agent-to-agent and not client-to-agent: "MCP defines how an agent talks to tools, ACP defines how an agent talks to an editor, and A2A defines how an agent talks to another agent" (paraphrased consensus across [Zylos Research](https://zylos.ai/research/2026-03-26-agent-interoperability-protocols-mcp-a2a-acp-convergence/), [tyk.io](https://tyk.io/learning-center/agent-protocols-a-complete-guide-to-mcp-a2a-and-acp/), Google's own framing above). This is inference-supported convention across secondary sources, not a single normative spec statement, but the pattern is consistent everywhere it appears.

## The IBM-ACP-into-A2A history (verify, don't conflate)

IBM Research launched its own "Agent Communication Protocol" (also abbreviated ACP) in 2025 as a REST-native alternative for BeeAI, unrelated at first to Zed's protocol. In August 2025 the Linux Foundation's LF AI & Data announced that IBM's ACP was merging into A2A: "the ACP team is winding down active development," contributing its technology and expertise directly into A2A, with IBM's Kate Blair joining the A2A Technical Steering Committee alongside Google, Microsoft, AWS, Cisco, Salesforce, ServiceNow, and SAP ([LFAI & Data announcement](https://lfaidata.foundation/communityblog/2025/08/29/acp-joins-forces-with-a2a-under-the-linux-foundations-lf-ai-data/), [i-am-bee/BeeAI discussion](https://github.com/orgs/i-am-bee/discussions/5)). BeeAI now uses adapters (`A2AServer`, `A2AAgent`) to bridge old ACP agents into A2A. This is the confirmed, verified origin of "ACP is becoming A2A" and it refers **only** to IBM/BeeAI's ACP. Zed's Agent Client Protocol is a separate, still-active, still-growing specification with no merger announced; conflating the two is the single biggest source of confusion in this space.

## The counterexample: ACP used in an agent-to-agent-like role

Zed's own ACP has an RFD for **proxy chains**, an in-progress proposal (prototype code exists in `symposium-acp`, upstreamed into the ACP Rust SDK, not yet finalized) that inserts intermediary "proxy" components between client and agent, routed through a central **conductor** that dispatches messages down a chain instead of proxies talking to each other directly ([agentclientprotocol.com RFD](https://agentclientprotocol.com/rfds/proxy-chains), [SymmACP write-up](https://smallcultfollowing.com/babysteps/blog/2025/10/08/symmacp/)). The RFD's own FAQ discusses extending this to "M:N topologies where a proxy communicates with multiple peers," explicitly flagged as not yet implemented. This is ACP creeping toward multi-component, agent-adjacent orchestration inside what is nominally the client-to-agent seat, the inverse direction from gemini-cli's clean split. Inference: this is a proposal-stage counterexample, not yet evidence of ACP replacing A2A for agent-to-agent work in production.

One further data point, weakly verified: a gateway product called "OpenClaw" reportedly offers both an ACP bridge (IDE-to-gateway-session) and a separate A2A gateway plugin (gateway-to-gateway) ([docs.openclaw.ai/cli/acp](https://docs.openclaw.ai/cli/acp), [win4r/openclaw-a2a-gateway](https://github.com/win4r/openclaw-a2a-gateway)). Its documentation gave no information on project maintainership or maturity, and most search results describing it were low-quality secondary/SEO content rather than primary sources; treat this one as unconfirmed color, not a load-bearing example.

## Implications for trogonai

The gemini-cli pattern (ACP local, A2A remote, MCP for tools) is the strongest, best-documented precedent in the wild and matches trogonai's existing crate boundary: ACP-over-NATS as the client-to-agent seat (agents-service facing a human or IDE client) and the a2a-nats family (ADR 0034) as the agent-to-agent seat for cross-service or cross-org delegation between trogon-gateway/trogon-decider/trogon-scheduler-adjacent agents. Nothing found suggests using A2A in ACP's seat is common or advisable. The proxy-chains RFD is worth tracking (inference: relevant if trogonai ever wants in-band prompt/response transformation or middleware within the ACP leg) but is pre-1.0 and should not be treated as a signal that ACP is absorbing A2A's job. The IBM-ACP merger is unrelated to Zed's ACP and should not be cited as evidence that trogonai's ACP dependency is being deprecated.

## Sources

- [Zed: Gemini CLI ACP Agent](https://zed.dev/acp/agent/gemini-cli)
- [gemini-cli acp-mode.md](https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/acp-mode.md)
- [geminicli.com ACP Mode docs](https://geminicli.com/docs/cli/acp-mode/)
- [Guillaume Laforge: Gemini CLI + IntelliJ via ACP](https://glaforge.dev/posts/2026/02/01/how-to-integrate-gemini-cli-with-intellij-idea-using-acp/)
- [gemini-cli subagents.md](https://github.com/google-gemini/gemini-cli/blob/main/docs/core/subagents.md)
- [gemini-cli PR #16013: remote agents](https://github.com/google-gemini/gemini-cli/pull/16013)
- [Devin docs: Agent Client Protocol](https://docs.devin.ai/desktop/acp)
- [goose discussion #4645: Adopt ACP](https://github.com/block/goose/discussions/4645)
- [goose discussion #7309: goose and ACP](https://github.com/aaif-goose/goose/discussions/7309)
- [LangChain: A2A endpoint in Agent Server](https://docs.langchain.com/langsmith/server-a2a)
- [CrewAI: Agent-to-Agent (A2A) Protocol docs](https://docs.crewai.com/en/learn/a2a-agent-delegation)
- [Google Developers Blog: Developer's Guide to AI Agent Protocols](https://developers.googleblog.com/developers-guide-to-ai-agent-protocols/)
- [LFAI & Data: ACP Joins Forces with A2A](https://lfaidata.foundation/communityblog/2025/08/29/acp-joins-forces-with-a2a-under-the-linux-foundations-lf-ai-data/)
- [i-am-bee/BeeAI discussion #5](https://github.com/orgs/i-am-bee/discussions/5)
- [agentclientprotocol.com RFD: Proxy Chains](https://agentclientprotocol.com/rfds/proxy-chains)
- [SymmACP blog post](https://smallcultfollowing.com/babysteps/blog/2025/10/08/symmacp/)
- [docs.openclaw.ai/cli/acp](https://docs.openclaw.ai/cli/acp) (weakly verified, low-quality secondary coverage)
- [win4r/openclaw-a2a-gateway](https://github.com/win4r/openclaw-a2a-gateway) (weakly verified)
