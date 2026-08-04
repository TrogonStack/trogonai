# Session store research corpus

This corpus is the research input behind the platform's Session Store
design: an industry study of how agent products persist, resume, list, and
retire session transcripts and session state. It follows the same method as
the [agent platform corpus](../agent-platform/index.md). Where a conclusion
here differs from an accepted record in the [ADR index](../../adr/index.md),
the ADR is authoritative.

## Method

The study runs in two stages, and both prompts are preserved so the scope and
evidence rules behind every artifact remain reproducible.

- [Stage one](./RESEARCH_PROMPT.md) produces a standalone dossier describing
  how a product persists, resumes, lists, and retires sessions.
- [Stage two](./RESEARCH_PROMPT_COMPARISON.md) consumes that dossier and
  produces a comparison against our event catalog and ADR#0035, weighting
  each product's evidence by how proven its store is rather than how popular
  the product is.

## Status

Synthesis complete for the first ten products, and the decision record exists
as draft
[ADR#0035: Session Store as a Decider Aggregate on NATS JetStream](../../adr/0035-session-store-decider-aggregate.md).
The fx artifacts (the dossier, the session detail JSON reference, and the
comparison against our event catalog) were added after the synthesis and are
not yet folded into it.

The corpus is being extended to twenty-eight products. The queue, its
ordering, and the verification state of each candidate are in the
[backlog](./backlog.md). Only fx has a stage-two comparison so far; it is the
reference implementation of that prompt.

## Product dossiers

Each product owns a directory under `products/`. The stage-one dossier is that
directory's `index.md`, the stage-two comparison is `vs-session-events.md`, and
any further evidence artifacts sit alongside them. The sibling
[ACP](../acp/index.md) and [agent platform](../agent-platform/index.md) corpora
keep a flat `products/*.md` because each of their products has a single
artifact; this corpus nests because every product here has at least two.

- [Claude Agent SDK and Claude Code](./products/claude-agent-sdk/index.md)
  - [Claude Agent SDK 0.3.220 session type snapshot and platform
    comparison](./products/claude-agent-sdk/session-types.md)
- [Codex CLI (OpenAI)](./products/codex-cli/index.md)
- [fx (Vercel)](./products/fx/index.md)
  - [fx session detail JSON reference](./products/fx/session-detail-json-reference.md)
  - [fx compared to our session event catalog](./products/fx/vs-session-events.md)
- [Gemini CLI (Google)](./products/gemini-cli/index.md)
- [Goose (Block)](./products/goose/index.md)
- [Grok Build](./products/grok-build/index.md)
- [Hermes (Nous Research)](./products/hermes-agent/index.md)
- [LangGraph (LangChain)](./products/langgraph/index.md)
- [OpenCode](./products/opencode/index.md)
- [T3 Code](./products/t3code/index.md)

## Synthesis

- [Synthesis: what the industry means by a "stored session"](./synthesis.md),
  the cross-product convergence and divergence analysis drawn from the
  dossiers above, organized around the append-log-vs-mutable-record spectrum.
