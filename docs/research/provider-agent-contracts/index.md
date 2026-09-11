# Provider agent contracts research corpus

This corpus is the research input behind the platform's credential plane: a
per-provider study of how the commercial agent platforms design three things,
**sessions**, **agents**, and **vault secrets**, studied independently of each
other and then read back against our contract. It follows the same method as
the [agent platform corpus](../agent-platform/index.md), narrowed to the axis
that corpus covers least: where a third-party credential lives, what it binds
to, and who holds it at the moment it is used. Where a conclusion here differs
from an accepted record in the [ADR index](../../adr/index.md), the ADR is
authoritative.

## Method

The [research prompt](./RESEARCH_PROMPT.md) is preserved so the scope and
evidence rules behind every dossier remain reproducible. Each provider was
researched in isolation, with no provider's findings visible to another
provider's run, so that agreement between two dossiers is evidence of
convergence rather than an artifact of the order they were written in.

## Product dossiers

- [Anthropic Claude Managed Agents](./products/anthropic-managed-agents.md)
- [OpenAI Agents API](./products/openai-agents-api.md)
- [OpenComputer Serverless Agents](./products/opencomputer-serverless-agents.md)
- [xAI platform](./products/xai-platform.md)

Three of the four providers also have dossiers in the
[agent platform corpus](../agent-platform/index.md), written against a
different question and frozen before this study ran:
[Claude Managed Agents](../agent-platform/products/claude-managed-agents.md),
[OpenAI Agents API](../agent-platform/products/openai-agents-api.md), and
[OpenComputer](../agent-platform/products/opencomputer.md). Those remain the
record of what the agent-platform study concluded at the time. This corpus is
the deeper read on sessions and credentials, and it supersedes neither. xAI has
no agent-platform dossier; its client-side coding agent is covered in the
[ACP corpus](../acp/products/grok-cli.md) instead, and this corpus records why:
there is no server-hosted agent product to write an agent-platform dossier
about.

## Combined proposal

- [Combined schema proposal](./combined-schema-proposal.md), the four dossiers
  read together against `proto/trogonai/agents/agents/v1alpha1` and
  `proto/trogonai/session/sessions/v1alpha1`. Analysis and proposal only; every
  change it argues for is gated on an ADR.

## Status

The four dossiers are complete for the evidence snapshot dated 2026-09-11. The
combined proposal is a draft proposal, not a decision: it closes some questions
that were open (egress substitution now has four independent implementations,
so it is no longer a live design choice) and opens others that each want their
own record.

One finding has since been recorded. Draft
[ADR#0063](../../adr/0063-agent-connection-declarations.md) takes the
declaration half of the proposal's §6.1 and adds `ConnectionDeclaration` to
`AgentDependencies`, citing this corpus for the two findings that fix its
shape: that material rotates while structural identity does not, which is why
that declaration is the one dependency in the contract that pins no version,
and that the secret never enters the agent process. Everything else here
remains a draft proposal rather than a decision, and the proposal's
[sequencing](./combined-schema-proposal.md#_8-sequencing) records what the
first pass through the contract changed about the order of the rest.

The headline findings, in the order they carry weight. Each is scoped to the
providers whose documentation supports it, because the weight of a finding in
this corpus is the number of independent designs that reached it, and a bullet
that reads as a universal rule destroys exactly that information:

- **Material rotates and structural identity does not.** Anthropic and OpenAI
  reached the same rule with no shared specification between them: the secret
  behind a credential may be replaced without changing the credential's id,
  authentication type, or bound server URL, while those three may not change
  at all.
- **Write-only is expressed by schema shape, not by a redaction rule.**
  OpenAI. The create type carries the secret and the read type simply has no
  field for it, so there is nothing to redact and no placeholder to leak
  through. Anthropic and OpenComputer are also write-only but do not document
  the property as a consequence of the type split.
- **Credentials attach to the session, not to the agent definition.** Anthropic
  (`vault_ids` on session create) and OpenAI (session-level vault attachment).
  This is the one place a pin-at-admission contract has to make an exception,
  because a pinned credential cannot be revoked. Note what it does not buy:
  OpenAI also permits a `credential_id` selector on an agent tool, and deleting
  a stored credential there revokes neither the provider-side token nor a
  session already running on it.
- **The secret never enters the agent process.** Anthropic and OpenComputer,
  plus the two open-source implementations already on file. Substitution
  happens at network egress from an opaque placeholder, and it is a set plus a
  strip, not a set alone. Scheme enforcement, redirect revalidation, and
  response isolation are undocumented in all four, so the mechanism as
  published bounds where a credential reaches rather than what a compromised
  agent can learn by using it.
- **The negative result is explanatory.** xAI has no vault and no
  provider-held MCP credential flow, and that is not a gap in xAI's design: a
  client-side agent holding the user's own credentials needs no server-side
  vault. The vault is a consequence of hosting someone else's agent against
  someone else's credentials. xAI is not credential-free, though, and the
  dossier inventories what it does have: inference API keys, management keys,
  ephemeral realtime tokens, mTLS certificates, an undocumented OAuth bearer
  path, and CLI credentials on the user's own disk.
- **One finding is single-source and is labeled as such.** Archive purges the
  payload and keeps the record while delete keeps nothing is Anthropic only.
  The proposal argues for reusing our existing erasure idiom on that basis,
  which is an argument from our own contract rather than from convergence.
