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
  produces a comparison against our event catalog and [ADR#0035](../../adr/0035-session-store-decider-aggregate.md), weighting
  each product's evidence by how proven its store is rather than how popular
  the product is.

## Status

Synthesis complete for the products it was frozen over, and the decision
record exists as draft
[ADR#0035: Session Store as a Decider Aggregate on NATS JetStream](../../adr/0035-session-store-decider-aggregate.md).
The fx artifacts (the dossier, the session detail JSON reference, and the
comparison against our event catalog) were added after the synthesis and are
not yet folded into it. The IronClaw dossier was also added after the
synthesis was frozen, but its evidence is folded in and marked inline
wherever it revised a frozen claim.

The corpus is still being extended. The queue, its
ordering, and the verification state of each candidate are in the
[backlog](./backlog.md). fx was the reference implementation of the stage-two
prompt; Cline, OpenHands, and Zed were the calibration batch that exercised it
against fresh dossiers, and the remaining comparisons follow their shape.

Stage-two comparisons exist for many of those products and are listed
under their dossiers below. The synthesis's frozen decision-time text has not
absorbed them, but its cross-corpus results are recorded in
[a closing section](./synthesis.md#stage-two-results-not-yet-absorbed-above)
rather than left in nobody's hands. Where a comparison's ranked recommendations
disagree with the frozen text, the comparison is the newer reading and the ADR is
still authoritative over both.

Every dossier and comparison listed below has been verified in two layers.
First, mechanically: each `path:line` citation is resolved against the pinned
tree by `verify-citations.py`, which distinguishes a missing file from an
ambiguous basename so that shorthand does not read as fabrication. Second, by
hand: each artifact's load-bearing claims are read back against source, because
a resolvable line number says nothing about whether the line supports the claim
made about it.

The mechanical layer is only trustworthy when it is run over the whole corpus
at once. Run per-artifact as each landed, it reported clean on files that were
not: a sweep of every artifact with a local clone found flattened paths, wrong
crates, and bare basenames in files that individual waves had already passed.
That sweep now resolves every citation it covers, with no missing file and no
out-of-range line, across every product whose source is checked out locally.
It does not cover the dossiers that predate it, which were verified by hand
only and therefore carry weaker mechanical guarantees; a dossier is inside the
sweep only if its product's source is checked out locally, so the boundary is
the local clone rather than a list kept here by hand.

The second layer is the one that earns its cost. It is what caught a migration
ratchet described as content-hashed when the source compares stored text, a
graph walk described as breadth-first when it is depth-first, a cascade
presented as complete when it stops one level from a root, and a fork credited
with a recursive delete it had vendored from a third codebase. Every one of
those cited a real line. None of them could have been caught mechanically.

The comparisons added two failure modes of their own, both now checked for
explicitly. The first is a citation that resolves to the right line number in
the wrong file, which reads as precise and is not: a commit-hash set was
attributed to `aider/commands.py:349`, where line 349 is an unrelated dirty
check, and a tree-entry envelope was attributed to a `types.ts` path that does
not exist in the package it names. The second is an absence established against
a file that is not there, which proves nothing: a field was reported as having
"zero matches" in a `compaction.ts` that is a directory, not a file. A
comparison may cite its own dossier, but by section link rather than line
number, and only for grep-established absences and for the dossier's own
conclusions; any claim about how code behaves carries a product source path
opened during the comparison.

A third failure mode surfaced in the Mastra dossier and is the hardest of the
citation defects to catch: a paraphrase presented inside quotation marks. A default-throw
message was quoted as "this is likely a bug -- all adapters should implement
this" when the source reads "This is likely a bug - all Mastra storage adapters
should implement resource support." The citation resolved, the claim it
supported was sound, and only reading the line back word for word showed the
quotation marks were doing work the source did not authorize. Quoted text is
now transcribed, never summarized inside quotes.

A fourth failure mode is not about citations at all but about what a section
leaves out, and it survived every check above because nothing it says is wrong.
Two dossiers document an entry's envelope and never open the payload inside it:
Google ADK's records every field `Event` declares while its content is inherited
from a superclass the dossier names only in passing, and Grok Build's names
`SessionUpdateEnvelope{timestamp, method, params}` as the source-of-truth log
line without opening `params`. Both quoted a type definition, as the prompt
asked; both quoted the wrong half. Rule 7 of the [stage-one
prompt](./RESEARCH_PROMPT.md) now says an envelope is not the payload, and the
two repairs are queued in the [backlog](./backlog.md), together with a queued
stage three that takes a provider rather than a product as its unit of study,
since nothing in the corpus enumerates what a `ProviderBlock` would carry.

## Product dossiers

Each product owns a directory under `products/`. The stage-one dossier is that
directory's `index.md`, the stage-two comparison is `vs-session-events.md`, and
any further evidence artifacts sit alongside them. The sibling
[ACP](../acp/index.md) and [agent platform](../agent-platform/index.md) corpora
keep a flat `products/*.md` because each of their products has a single
artifact; this corpus nests because every product here has at least two.

- [Aider](./products/aider/index.md)
  - [Aider compared to our session event catalog](./products/aider/vs-session-events.md)
- [Amazon Q Developer CLI](./products/amazon-q/index.md)
  - [Amazon Q Developer CLI compared to our session event
    catalog](./products/amazon-q/vs-session-events.md)
- [AWS Strands Agents](./products/aws-strands/index.md)
  - [AWS Strands Agents compared to our session event
    catalog](./products/aws-strands/vs-session-events.md)
- [Claude Agent SDK and Claude Code](./products/claude-agent-sdk/index.md)
  - [Claude Agent SDK 0.3.220 session type snapshot and platform
    comparison](./products/claude-agent-sdk/session-types.md)
- [Cline](./products/cline/index.md)
  - [Cline compared to our session event catalog](./products/cline/vs-session-events.md)
- [Codex CLI (OpenAI)](./products/codex-cli/index.md)
- [Continue](./products/continue/index.md)
  - [Continue compared to our session event catalog](./products/continue/vs-session-events.md)
- [Crush (Charm)](./products/crush/index.md)
  - [Crush compared to our session event catalog](./products/crush/vs-session-events.md)
- [DeepSeek Harness](./products/deepseek-harness/index.md)
  - [DeepSeek Harness compared to our session event
    catalog](./products/deepseek-harness/vs-session-events.md)
- [fx (Vercel)](./products/fx/index.md)
  - [fx session detail JSON reference](./products/fx/session-detail-json-reference.md)
  - [fx compared to our session event catalog](./products/fx/vs-session-events.md)
- [Gemini CLI (Google)](./products/gemini-cli/index.md)
- [Google ADK](./products/google-adk/index.md)
  - [Google ADK compared to our session event catalog](./products/google-adk/vs-session-events.md)
- [Goose (Block)](./products/goose/index.md)
- [Grok Build](./products/grok-build/index.md)
- [Hermes (Nous Research)](./products/hermes-agent/index.md)
- [IronClaw (NEAR AI)](./products/ironclaw/index.md)
- [LangGraph (LangChain)](./products/langgraph/index.md)
- [Letta](./products/letta/index.md)
  - [Letta compared to our session event catalog](./products/letta/vs-session-events.md)
- [Mastra](./products/mastra/index.md)
  - [Mastra compared to our session event catalog](./products/mastra/vs-session-events.md)
- [OpenAI Agents SDK](./products/openai-agents-sdk/index.md)
  - [OpenAI Agents SDK compared to our session event
    catalog](./products/openai-agents-sdk/vs-session-events.md)
- [OpenCode](./products/opencode/index.md)
- [OpenHands](./products/openhands/index.md)
  - [OpenHands compared to our session event catalog](./products/openhands/vs-session-events.md)
- [Pi](./products/pi/index.md)
  - [Pi compared to our session event catalog](./products/pi/vs-session-events.md)
- [SWE-agent](./products/swe-agent/index.md)
  - [SWE-agent compared to our session event catalog](./products/swe-agent/vs-session-events.md)
- [T3 Code](./products/t3code/index.md)
- [Void](./products/void/index.md)
  - [Void compared to our session event catalog](./products/void/vs-session-events.md)
- [Zed](./products/zed/index.md)
  - [Zed compared to our session event catalog](./products/zed/vs-session-events.md)

## Fork deltas

Forks of a product already in the corpus get a delta report instead of a
dossier: one question, what diverged from upstream's store, with paths.
"Nothing diverged" is a complete answer. The rationale and the queue are in the
[backlog](./backlog.md) under Wave 5.

These are not lesser artifacts. Roo Code was queued as a presumed restatement
of Cline and turned out to hold the corpus's cleanest counter-example on
cascade semantics: it recurses over the full child-task tree where its own
upstream stops at one level.

- [Roo Code, diverged from Cline](./products/roo-code/index.md)
- [Kilo Code, diverged from Cline via Roo Code](./products/kilo-code/index.md)
- [Qwen Code, diverged from Gemini CLI](./products/qwen-code/index.md)

Kilo Code is the limit case of the form. It did not evolve the store it
inherited, it replaced the whole subsystem with a vendored copy of
[OpenCode](./products/opencode/index.md)'s, so its delta answers "everything
diverged" and its evidence belongs to a lineage the fork question did not ask
about. Its `// kilocode_change` markers are what separate OpenCode's design
decisions from Kilo's own patches, and without that discriminator the report
would have credited Kilo with a cascade design it merely vendored.

## Synthesis

- [Synthesis: what the industry means by a "stored session"](./synthesis.md),
  the cross-product convergence and divergence analysis drawn from the
  dossiers above, organized around the append-log-vs-mutable-record spectrum.
