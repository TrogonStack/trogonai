# Research Prompt: how {PRODUCT} compares to our Session Store

Reusable prompt for stage two of the session-store study. Stage one is
[RESEARCH_PROMPT](./RESEARCH_PROMPT.md), which produces a standalone dossier
describing how a product stores sessions. This prompt consumes that dossier
and produces a comparison against our own design, plus a ranked set of
changes we should consider making.

Run once per product. Output goes to
`docs/research/session-store/products/{slug}/vs-session-events.md` and is
linked from `index.md` under its stage-one dossier.

[fx compared to our session event catalog](./products/fx/vs-session-events.md)
is the worked reference implementation of this prompt. Match its shape.

## Preconditions

Do not run this prompt until the stage-one dossier for {PRODUCT} exists and
its claims are pinned to a commit or a dated document snapshot. A comparison
built on an unverified dossier inherits its errors and launders them into a
recommendation.

## Inputs

Read all of these before writing anything:

- The stage-one dossier: `./products/{slug}/index.md`.
- Our decision record:
  [ADR#0035: Session Store as a Decider Aggregate on NATS JetStream](../../adr/0035-session-store-decider-aggregate.md).
  This is authoritative. Where the dossier's conclusion conflicts with an
  accepted ADR, note the difference; do not override it.
- Our event catalog: `proto/trogonai/session/sessions/v1alpha1/`. Read the
  actual `.proto` files. Do not compare against a remembered version of the
  catalog.
- The cross-product [synthesis](./synthesis.md), so a recommendation already
  argued there is cited rather than re-derived.

## Break-change latitude

The catalog is `v1alpha1` and ADR#0035 is a draft. **Breaking changes are on
the table.** A recommendation must not be watered down into an additive
half-measure merely to preserve wire compatibility.

What this does *not* mean is that breaking changes are free. Every
recommendation must state its blast radius explicitly:

- **Additive** — new field, new event type, new optional value. No migration.
- **Breaking, cheap** — rename, retype, or field removal with no persisted
  data to carry forward, or a mechanical regeneration.
- **Breaking, expensive** — changes the meaning of already-persisted events,
  requires a replay/rewrite, or splits or merges an existing event type.
- **Breaking the decision, not the schema** — contradicts a numbered decision
  in ADR#0035. Name the decision by number. These are the most valuable
  findings and the most expensive to act on.

Prefer the honest expensive recommendation over the dishonest cheap one.
State the cost; let the ADR owner make the trade.

## Maturity weighting

Weight evidence by how proven the **store** is, not by how popular the
product is. A 48k-star product whose sessions are an unversioned markdown
log is weak evidence. A 2k-star vendor CLI with eight SQL migrations is
strong evidence, because its schema demonstrably survived contact with
shipped users.

Score each axis 0-3 and record the evidence inline. Do not report a bare
number without the artifact that justifies it.

| Axis | What earns a high score | Evidence to cite |
| --- | --- | --- |
| **Evolution scars** | The store format changed under load and carried its data forward | Migration files, schema-version fields, legacy-format sniffing, back-compat read paths, format-version constants |
| **Operational age** | The store has been in the field long enough to hit real failure modes | First commit touching the store, not repo creation date; issues reporting corruption, growth, or lock contention, and their fixes |
| **Exposure** | Real users depend on resume working across crashes, upgrades, and hosts | Vendor-shipped distribution, paid product, or adoption scale; multi-host or network-filesystem handling in the code |
| **Design independence** | The store is an original design, not inherited from an upstream fork | Whether the store code diverges from the fork parent, with paths |

Sum to a 0-12 **store maturity score**. Record it in the comparison's front
matter. It is not a ranking of products; it is the weight the reader should
place on this product's answer when it disagrees with another product's.

Two rules follow from the score:

1. When products disagree, the higher-scoring store's answer is the default,
   and the comparison must say why the lower-scoring one diverged (deliberate
   trade-off, immaturity, or different problem).
2. A recommendation supported only by stores scoring under 6 must be labelled
   **thin evidence** and must not be presented as an industry norm.

## Research questions

### 14. Comparison against our catalog

- **The one structural difference everything else follows from.** Most
  comparisons reduce to a single divergence (commit granularity, identity
  model, mutability, ownership of derived state) that explains the rest of
  the diffs as consequences. Find it and lead with it. If there genuinely
  isn't one, say so rather than manufacturing one.
- **Fact-by-fact mapping.** For every durable field or entry type in the
  product's store, name our equivalent event or field, or record that we have
  none. Use a table. Include the reverse direction: what we record that they
  do not, and whether their omission looks deliberate.
- **Semantic mismatches.** Where both sides have a nominal equivalent that
  means something different (a "session id" that is a path in one and an
  opaque id in the other; a "checkpoint" that is a marker in one and a
  snapshot in the other), call it out. These are more dangerous than gaps
  because they survive a naive mapping.
- **Where our design is already ahead.** Required, not optional. A comparison
  that only finds gaps is a comparison that was not read critically.

### 15. What we should consider changing

Each recommendation is a numbered subsection, ordered most-consequential
first, and must carry all of:

- **The change**, stated concretely against a named `.proto` file, event
  type, or ADR#0035 decision number.
- **The evidence anchor**: the product, its store maturity score, and the
  `path:line` or quote that supports it. No anchor, no recommendation.
- **Blast radius**, using the four categories above.
- **Why it is a good idea, or why it is not.** A recommendation may conclude
  *do not do this*. Recording a rejected change with its reasoning is as
  valuable as recording an accepted one, and stops it being re-proposed.
- **What it costs us** beyond the migration: added write-path work, larger
  events, a new projection to maintain, a new failure mode.

Then three closing buckets, all required:

- **Trade-offs, not gaps.** Differences that are defensible on both sides.
  Say what each side bought and paid.
- **What not to copy.** Patterns present in the product that we should
  explicitly reject, with the reason. This section prevents a future reader
  mistaking the dossier's description for endorsement.
- **Open questions for the ADR.** Anything the comparison surfaced that the
  ADR does not currently answer, phrased as a question the ADR owner can
  decide.

### 16. Feed the two open gaps

The synthesis concludes that the industry has approximated the event-sourced
session store everywhere, and that our remaining work is the two gaps nobody
has closed: **subagent cascade semantics** and **retention on an unbounded
log**. Every comparison must answer both explicitly, even if the answer is
"this product has no position on it":

- What does this product do when a parent session is deleted, rewound, or
  crashes while a child session is live? Quote the code path, not the docs.
- What stops this product's durable record from growing without bound? If
  nothing does, find the issue reports where that became a user-visible
  problem.

## Method

1. Every claim about the product traces to the stage-one dossier or to a
   pinned `path:line`. Every claim about our design traces to a `.proto` file
   or an ADR#0035 decision number. Assertions with neither do not ship.
2. Record the retrieval date with `date +%F`; never guess it.
3. Quote our own proto fields exactly. The catalog moves; a paraphrase from
   memory will silently go stale.
4. Mark inference as inference. The dossiers keep description and conclusion
   separate on purpose; keep that discipline here.
5. Put anything unresolved under **Open questions** rather than resolving it
   with a guess.

## Output skeleton (per product file)

```markdown
# {PRODUCT} compared to our session event catalog

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT_COMPARISON](../../RESEARCH_PROMPT_COMPARISON.md).
Stage-one dossier: [{PRODUCT}](./index.md).
Compared against `proto/trogonai/session/sessions/v1alpha1/` and ADR#0035 on YYYY-MM-DD.

**Store maturity: N/12** — evolution scars N/3 ({evidence}), operational age
N/3 ({evidence}), exposure N/3 ({evidence}), design independence N/3 ({evidence}).

## The one structural difference everything else follows from
## Mapping
## What we should consider changing
### 1. {change}
### 2. {change}
## What our design already does better
## Trade-offs, not gaps
## What not to copy
## The two open gaps
### Subagent cascade
### Retention on an unbounded log
## Open questions for the ADR
```
