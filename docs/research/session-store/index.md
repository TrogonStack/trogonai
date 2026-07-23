# Session store research corpus

This corpus is the research input behind the platform's Session Store
design: an industry study of how agent products persist, resume, list, and
retire session transcripts and session state. It follows the same method as
the [agent platform corpus](../agent-platform/index.md). Where a conclusion
here differs from an accepted record in the [ADR index](../../adr/index.md),
the ADR is authoritative.

## Method

The [research prompt](./RESEARCH_PROMPT.md) is preserved so the shared scope
and evidence rules behind each product dossier remain reproducible.

## Status

Synthesis complete. Nine product dossiers and the cross-product synthesis
exist; the decision record has not run yet.

## Product dossiers

- [Claude Agent SDK and Claude Code](./products/claude-agent-sdk.md)
- [Codex CLI (OpenAI)](./products/codex-cli.md)
- [Gemini CLI (Google)](./products/gemini-cli.md)
- [Goose (Block)](./products/goose.md)
- [Grok Build](./products/grok-build.md)
- [Hermes (Nous Research)](./products/hermes-agent.md)
- [LangGraph (LangChain)](./products/langgraph.md)
- [OpenCode](./products/opencode.md)
- [T3 Code](./products/t3code.md)

## Synthesis

- [Synthesis: what the industry means by a "stored session"](./synthesis.md),
  the cross-product convergence and divergence analysis drawn from the
  dossiers above, organized around the append-log-vs-mutable-record spectrum.
