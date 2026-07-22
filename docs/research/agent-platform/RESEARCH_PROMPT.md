# Research Prompt: What "agent" means in {PRODUCT}

Reusable prompt for the agent-definition study. Run once per product. Output
goes into `docs/research/agent-platform/products/{slug}.md`, following the
section skeleton below. Add the dossier to `index.md` when done.

## Task

Research how **{PRODUCT}** ({URLS}) defines and models an "agent". The goal is
the operational definition, not marketing copy: what the noun refers to in
their API, data model, config files, and runtime. This feeds a cross-product
synthesis on what the industry actually means by "agent".

## Research questions

Answer every question the sources can support. Quote primary sources; mark
gaps as gaps instead of guessing.

### 1. The agent noun

- What do the docs/API literally say an agent is? Capture exact quotes.
- Is the agent a persistent entity (identity, record, resource with an ID) or
  an ephemeral one (a process, a run, an invocation)?
- Which conceptual model fits best: agent-as-identity, agent-as-config,
  agent-as-process, agent-as-session, agent-as-workspace, other?

### 2. Subagents

- Does the product have subagents (child agents, delegated agents, task
  agents, workers)? What are they called?
- How is a subagent created: declared ahead of time (config/registry) or
  spawned dynamically at runtime by the parent?
- What does a subagent inherit from its parent (model, tools, permissions,
  context, filesystem, credentials) and what is isolated?
- Can subagents nest? Is there a depth or fan-out limit?
- How do parent and subagent communicate: return value only, message passing,
  shared state, shared filesystem?

### 3. Configuration surface

- What is configurable on an agent: model, system prompt, instructions, tools,
  permissions, sandbox/environment, memory, secrets/credentials, triggers,
  schedule, resource limits?
- Where does configuration live: files in a repo, API objects, dashboard, all
  of the above? What format (YAML frontmatter, JSON, code)?
- **Why** does the product expose each knob? Look for the stated rationale
  (safety, cost, reproducibility, multi-tenancy, team sharing) rather than
  inferring one.

### 4. Binding time

- When is each piece of configuration bound: at definition time (static file,
  registered template), at session/run creation, or mutable mid-run?
- Can an agent's definition change while instances of it are running? What
  happens to in-flight work?
- Is there versioning of agent definitions?

### 5. Relationships between nouns

- Map the product's nouns and their cardinality: agent, subagent, session,
  run, task, thread, sandbox, workspace, workflow, tool. Which contains which?
  Which references which?
- Specifically: agent-to-session (one agent, many sessions? session owns the
  agent?), agent-to-sandbox (does an agent imply an environment?), and
  agent-to-subagent (ownership, lifetime coupling: does the parent's death
  kill the child?).

### 6. Lifecycle

- How is an agent created, started, paused/hibernated, resumed, and destroyed?
- Who owns the loop: does the product run the agent loop (managed) or does the
  customer's code drive it (bring-your-own-loop)?
- What persists across runs: memory, filesystem, conversation history?

### 7. The implicit definition

- Based on all the above, what makes something "an agent" in this product,
  versus a plain LLM call or a script? State it in one or two sentences, in
  our words, clearly marked as our inference.

## Method

1. Primary sources first: official docs, API reference, SDK source, config
   schemas, official repos. Secondary sources only to triangulate.
2. Capture each exact quote as a checked-in source excerpt. Record its direct
   URL, document section or repository symbol, source version or commit when
   available, and retrieval date. Also record an archive or snapshot URL when
   one exists and a content digest when the source publishes or permits one.
   These evidence records must remain auditable if the live URL changes.
3. Record the retrieval date with `date +%F`; never guess it.
4. Stay on the operational definition. Ignore pricing, marketing claims, and
   features unrelated to the agent model.
5. Fill the product skeleton sections in order; do not invent sections. Put
   anything the sources leave unanswered under **Open questions**.

## Output skeleton (per product file)

```markdown
---
title: "{PRODUCT}: what 'agent' means"
source_urls: [...]
retrieved: YYYY-MM-DD
status: done
---

# {PRODUCT}: what "agent" means

Part of [Agent platform research corpus](../index.md).
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).

## The `agent` noun (primary-source quotes)
## Subagents
## Configuration surface (what, where, why)
## Binding time
## Relationships between nouns
## Lifecycle
## What makes it "an agent" here (our inference)
## Open questions
```
