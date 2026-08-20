# Agent instructions research corpus

This corpus preserves the frozen research input behind the platform's decision
on how agent instructions are owned, shaped, and carried on the wire, together
with clearly marked later evidence. The original study surveyed what shipping
harnesses accept as standing agent instructions and how prompt-management
products model prompt content as of 2026-07-30. Where a conclusion here differs
from an accepted record in the [ADR index](../../adr/index.md), the ADR is
authoritative.

The decision this corpus feeds is
[ADR#0043: Agent Instructions Ownership and Shape](../../adr/0043-agent-instructions-ownership-and-shape.md).

## Method

The original findings were gathered from official documentation and, where
available, product source code retrieved 2026-07-30. The DeepSeek Harness
addition is post-decision evidence retrieved 2026-08-20 and is labeled with its
pinned release in the survey. Documentation is mutable, so each document names
the pages, repositories, and source paths checked rather than implying a fixed
product version. Facts that could not be confirmed against a primary source
are marked unverified.

## Documents

- [Harness survey](./harness-survey.md): what Claude, Codex, DeepSeek Harness,
  Cursor, Grok, Gemini CLI, OpenCode, Amp, Cline, Goose, Devin, and Aider
  accept as standing instruction input, and the five shapes the ecosystem
  reduces to.
- [Prompt management](./prompt-management.md): how Langfuse, LangChain,
  and the provider APIs model prompt content, and why prompt management
  needs shapes that agent definitions do not.

## Related corpora

- [Agent platform research](../agent-platform/index.md), the industry
  study behind the agent entity and revision model. The instruction
  survey here extends that corpus's charter-content questions (Q16, Q17)
  with an input-shape census.
