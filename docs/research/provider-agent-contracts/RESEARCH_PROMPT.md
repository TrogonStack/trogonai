# Research Prompt: how {PROVIDER} designs sessions, agents, and secrets

Reusable prompt for the provider agent contracts study. Run once per provider.
Output goes into
`docs/research/provider-agent-contracts/products/{slug}.md`, following the
section skeleton below. Add the dossier to `index.md` when done.

This prompt was reconstructed from the study it produced rather than written
before it, so treat the skeleton and the method rules as the reproducible
part and the wording as a restatement. The four dossiers in `products/` were
researched against it in parallel, each provider independently, with no
provider's findings visible to another provider's run.

## Task

Research how **{PROVIDER}** ({URLS}) designs three things: **sessions**,
**agents**, and **vault secrets**. Study each provider in isolation. The point
is not a feature matrix but the contract each provider actually commits to in
its API: which resources exist server side, what is pinned at creation versus
resolved per use, and who holds a secret at the moment it is used.

The three axes carry equal weight. Provider documentation is uneven here:
sessions and agents are usually documented well and secrets are usually
documented thinly or in a corner of the site, which is exactly why the secret
axis needs deliberate attention rather than whatever the overview page
volunteers.

A provider that has none of these is a result, not a dead end. Record the
absence, and say what the absence explains.

## Research questions

Answer every question the sources can support. Quote primary sources; mark
gaps as gaps instead of guessing.

### 1. Resource model

- Enumerate the actual server-side resources, not the four nouns the overview
  page names. Give each one its endpoint, its `object` discriminator if it has
  one, and its id prefix if it has one.
- Draw the containment and reference structure: what scopes what, and which
  references are by id, by id plus version, or by content digest.
- Where a documented concept turns out not to be a resource, say so.

### 2. Agents

- What does the provider literally say an agent is? Capture exact quotes.
- Is the agent definition stored, versioned, activated, deployed, or none of
  these? If versioned, what does a version cover and what does it exclude?
- What is in the definition and what is deliberately outside it (credentials,
  live state, routing, session input)?
- Delegation: is there a subagent concept? Is a subagent addressable, is it a
  session of its own, and is the delegation depth bounded?

### 3. Sessions

- What is the durable session: an append-only event log, a mutable record, a
  chained per-request object, nothing at all?
- What does a session pin at creation, and is the pin by id or by id plus
  version? What happens to a running session when a pinned resource changes
  underneath it?
- Ordering: what guarantees it, and is the ordinal exposed to callers?
- Event cursors, stream replay, and resumability. Name the endpoint each
  claim comes from, not just the page.
- Lifecycle and terminal states: cancel, expire, archive, delete. What does
  each one keep and what does it purge?

### 4. Vault secrets

- Is there a server-side vault resource at all? If not, how are third-party
  credentials supplied, and on what cadence?
- Is a stored secret readable back? If it is write-only, is that expressed in
  the schema shape (a create type and a read type that differ) or by a
  redaction rule applied to one type?
- What does the secret bind to: a vault, an agent, a session, a destination
  URL, a method and path? How tightly, and is the binding checked at use time
  or at attach time?
- Attach point: does a credential attach to the agent definition, the session,
  or the request? Does it propagate to delegates?
- Does the agent process ever hold the plaintext? If not, where is the
  substitution performed, and what exactly is substituted (set, strip, both)?
- Rotation: what may change without changing the credential's identity, and
  what may not?
- Deletion and archival: what happens to the secret payload versus the
  credential record.

### 5. Identity and versioning

- How is each resource identified, and is identity structural (a digest over
  bytes) or assigned (a server-minted id)?
- What rotates and what is immutable? State the rule the provider is actually
  enforcing, in their words where possible.

### 6. Notable design decisions

- Call out the choices that look deliberate rather than incidental, including
  ones we would not make. Say what each one buys the provider.

## Method

1. Primary sources first: official docs, API reference, machine-readable
   specs, SDK source. Secondary sources only to triangulate.
2. Prefer the markdown twin of a documentation page when the site publishes
   one (append `.md` to the page URL). Where the twin 404s, fetch the HTML and
   convert it, and say which pages needed that.
3. Use the site's own index (`llms.txt`, `sitemap.xml`, `openapi.json`) to
   enumerate pages rather than guessing URLs. When an index is incomplete,
   record that it is incomplete and name what it omits, because an incomplete
   index is itself a finding about how well documented the surface is.
4. **Record the endpoint path alongside every extracted API fact.** A
   qualifier like "list endpoints return one page at a time" scopes to the
   endpoints it names, not to the page's topic. Attaching such a sentence to
   the topic instead of to its endpoint produced a false cursor claim in an
   early draft of this study: an `after` cursor documented for the item and
   turn list endpoints was written up as belonging to the event stream, which
   exposes no cursor at all.
5. **Quotation marks mean transcribed, never summarized.** Text inside quotes
   must match the source character for character, including its punctuation.
   Paraphrase freely, but then drop the quotes and cite the URL. This is why
   the dossiers here contain em dashes inside quoted vendor text and nowhere
   else.
6. Probe negatives before asserting them. When a capability appears in a spec
   with no documentation page, try the plausible URLs and record which ones
   returned 404, so "undocumented" is an observation rather than an
   assumption.
7. Record the retrieval date with `date +%F`; never guess it.
8. Rank a design claim by how many independent implementations reached it.
   Two providers with no shared specification arriving at the same rule is
   evidence; one provider doing something is a data point.
9. Where a conclusion here would differ from an accepted record in the
   [ADR index](../../adr/index.md), the ADR is authoritative; note the
   difference rather than overriding it.

## Output skeleton (per product file)

```markdown
# {PROVIDER}: Sessions, Agents, Vaults

Part of the [provider agent contracts research corpus](../index.md).
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot retrieved YYYY-MM-DD. {anchors and version pin, or an
explicit statement that the provider publishes no version pin}

## Resource model
## Agents
## Sessions
## Vaults and secrets
## Identity and versioning
## Notable design decisions
## Gaps and open questions
## Sources
```

Rename a section when the provider's own model makes the generic title
misleading, and keep the position: the xAI dossier carries
`Agents (or the absence of them)` and `Session and conversation state` for
exactly that reason.

## Second stage

Once every provider dossier exists, the combined stage reads all of them
together and produces
[combined-schema-proposal.md](./combined-schema-proposal.md): the convergences,
the divergences, the negative results, and a proposal read back against
`proto/trogonai/agents/agents/v1alpha1` and
`proto/trogonai/session/sessions/v1alpha1`. That stage may not introduce a
claim that is not already sourced in a dossier.
