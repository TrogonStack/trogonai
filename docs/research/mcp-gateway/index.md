# MCP gateway research corpus

This corpus is the research input behind the platform's MCP edge: how
production platforms put one gateway between agents and many tool servers,
what a registry behind that gateway owns, and which of those responsibilities
this repository has already decided or still owes. Where a conclusion here
differs from an accepted record in the [ADR index](../../adr/index.md), the
ADR is authoritative.

## Method

Each dossier quotes its primary source, dates the retrieval, and marks every
inference as ours. Sources with no public repository are studied through
their published engineering write-ups and documentation. Gateway material
already captured in other corpora is linked rather than restated.

## Product dossiers

- [Uber MCP Gateway](./products/uber-mcp-gateway.md)

Gateway evidence held in neighbouring corpora:

- [Bedrock AgentCore](../agent-platform/products/bedrock-agentcore.md),
  AgentCore Gateway as a virtual MCP server over targets.
- [OpenAI Agents API](../agent-platform/products/openai-agents-api.md),
  hosted `mcp` tool, `allowed_tools`, and deferred tool loading.
- [xAI platform](../provider-agent-contracts/products/xai-platform.md),
  caller-held MCP tokens and `tool_search`.
- [Guild](../agent-platform/products/guild.md), progressive disclosure of
  skills.

## Synthesis

[Synthesis](./synthesis.md): convergence and divergence across the dossiers,
the decisions this repository still owes with the ADRs that constrain each
one, and the commitments it already holds that the industry evidence does
not.
