# ACP research corpus

The Agent Client Protocol (ACP) is Zed's open protocol for the client-to-agent
seat: an editor, IDE, gateway, or any other host drives a coding agent through
a standardized JSON-RPC surface, turning the N x M client/agent integration
problem into N + M. This corpus preserves the frozen research input behind the
platform's ACP-related decisions and clearly marked later evidence: an
industry study of ACP's protocol contract, ecosystem adoption, and product
integrations, plus the roadmap analysis that followed it. Where a conclusion
here differs from an accepted record in the [ADR index](../../adr/index.md) or
from the current spec
position in [ACP Conformance](../../architecture/acp-conformance.md), the ADR
or the conformance document is authoritative.

## Method

The [research prompt](./RESEARCH_PROMPT.md) is preserved so the shared scope,
disambiguation rules, and evidence bar behind each product dossier and deep
dive remain reproducible.

## Product dossiers

The original study covered fifteen products. Later product dossiers are
marked as post-synthesis evidence rather than rewritten into the frozen
decision-time input. Every dossier examines native implementation vs.
adapter, process lifecycle ownership, headless invocation and auth, and
channel mapping where relevant.

- [Buzz](./products/buzz.md)
- [Claude Code](./products/claude-code.md)
- [Cline](./products/cline.md)
- [Codex CLI](./products/codex-cli.md)
- [Cursor](./products/cursor.md)
- [DeepSeek Harness](./products/deepseek-harness.md)
- [Devin](./products/devin.md)
- [Gemini CLI](./products/gemini-cli.md)
- [Goose](./products/goose.md)
- [Grok CLI](./products/grok-cli.md)
- [Hermes Agent](./products/hermes-agent.md)
- [JetBrains](./products/jetbrains.md)
- [NetClaw](./products/netclaw.md)
- [OpenClaw](./products/openclaw.md)
- [OpenCode](./products/opencode.md)
- [Zed](./products/zed.md)

## Synthesis

- [Synthesis: the ACP protocol contract and ecosystem](./synthesis.md), the
  cross-cutting analysis of the wire contract, the v1-vs-v2 breaking-change
  diff, governance and adoption, and the integration patterns drawn from the
  dossiers above.

## Decision record

- [ACP and trogonai: fit and roadmap](./decision-record.md), the gap analysis
  and recommended build sequencing that turned the synthesis into a
  directional plan for hosting external ACP agent CLIs.

## Deep dives

Component-level designs that support the decision record's recommended
`acp-host` build:

- [Rust Crate Inventory](./rust-crates.md)
- [Tier 2 Client Profiles](./tier2-profiles.md)
- [Host Role and Invocation Mechanics](./host-role-and-invocation.md)
- [Channel Mapping](./channel-mapping.md)
- [Channel Bridge Mechanics](./bridge-mechanics.md)
- [File and Media Pipeline](./file-media-pipeline.md)
- [Permission Decision Point](./permission-decision-point.md)
- [Credential Injection at Spawn](./secrets-at-spawn.md)
- [Media Store Decision](./media-store.md)
- [Sandboxed Per-Session Workspaces](./sandboxed-workspaces.md)
- [ACP vs A2A](./acp-vs-a2a.md)
