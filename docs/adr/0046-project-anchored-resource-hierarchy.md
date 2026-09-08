---
number: "0046"
slug: project-anchored-resource-hierarchy
status: accepted
date: 2026-08-05
---

# ADR#0046: Project-Anchored Resource Hierarchy for the Credential Platform

## Context

The credential vault and API key platform needs one canonical owner boundary.
The shipped gateway slice already bakes `CredentialOwnerId` into OpenBao paths
(`trogonai/{owner_id}/credentials/{credential_id}`), generated credential ids
(`openbao:{owner}:{scope}:{kind}`), event stream routing, and the credential
state protos. Whatever "owner" means, changing its meaning later migrates
every one of those surfaces at once, so the boundary must be chosen before the
public management API or the broader domain model hardens around it.

The candidates were workspace, organization, project, tenant, or user. The
three hyperscalers embody three answers, and their histories matter more than
their marketing:

- AWS hardened the wall it happened to have. The account predates IAM; the
  ecosystem converged on many-accounts-per-company and AWS formalized it
  after the fact. Isolation is structural and strong, and every interior
  structure is ceremony bolted around a retail-era boundary.
- Azure accreted a layer per business era: the tenant is the enterprise
  identity directory, the subscription is a procurement artifact, resource
  groups patched subscriptions for deployment lifecycle, management groups
  patched governance above them.
- GCP designed top-down. The project was the API-console unit from the
  beginning (billing attachment, quota, IAM anchor), and when organizations
  and folders arrived years later they slotted in above without renaming a
  single resource, because resource names had always anchored at the
  project: `projects/{project}/secrets/{secret}/versions/{version}`.

That last property is the decisive one. GCP resource names anchor at the most
stable container and never embed the hierarchy above it, so reorganizing
companies, teams, or billing never rewrites a stored path. GCP Secret
Manager's model (secret plus per-version enabled/disabled/destroyed states)
is also nearly isomorphic to the credential version lifecycle this platform
already ships, which makes its naming grammar a proven reference rather than
an invention.

Isolation strength does not force the choice. A hard AWS-style wall and a
GCP-style connected hierarchy converge in achievable capability; they differ
in default posture and in which misconfiguration class bites. Every rung of
the isolation ladder (scoped policies on shared infrastructure, per-owner
OpenBao namespaces and keys, dedicated cells) lives beneath the resource
model and changes no name, path, or stream.

## Decision

### 1. Organization over project; the project is the owner boundary

The hierarchy is two levels: organization, then project. The project is the
unit that owns credential vaults, credentials, integrations, API keyspaces,
and quotas. `CredentialOwnerId` in the shipped slice reads as a project id,
and the broader owner value object is a project id.

### 2. Names anchor at the project and never embed the organization

The project id is an identifier in the [ADR#0040](./0040-contract-field-vocabulary.md)
sense: rigid, opaque, minted once, never reused. Human-facing naming is a
`display_name` on the project record, never part of a path. Resource names,
OpenBao paths, event stream routing, and generated credential ids contain the
project id and nothing above it, so re-parenting a project to a different
organization is an IAM and billing event, not a storage migration.

### 3. Public resource names are parent-scoped

The public management API adopts AIP-style parent-scoped resource names rooted
at the project: `projects/{project}/credential-vaults/{vault}`,
`projects/{project}/credentials/{credential}`, and so on. Flat, unparented
names such as `/v1/credentials` are superseded. The parent is also an
authorization statement: admission derives the expected project from the
authenticated caller context
([ADR#0050](./0050-signed-first-caller-authentication.md),
[ADR#0051](./0051-fully-bound-request-signing.md)) and rejects a request whose
`{project}` does not match it.

### 4. Environments are attributes, not hierarchy levels

Environment (production, preview, development) is a field on vaults and
credentials, the way Vercel scopes environment variables, not a container in
the hierarchy.

### 5. Isolation is deployment posture beneath the model

Tenant isolation is implemented under the unchanged resource model, in
rungs: scoped OpenBao policies on shared infrastructure first, per-project or
per-organization OpenBao namespaces, mounts, and encryption keys when a
customer tier demands it, dedicated cells at the top. GCP's own guardrail
retrofits (organization policy constraints, deny policies, service
perimeters) are the reference list for the constraint plane this platform
will eventually place above per-project grants.

### 6. Deferred layers

Organizations ship later as a pure IAM and billing plane above projects.
Folder-style nesting is deferred until enterprise demand exists; the naming
rule in section 2 guarantees it can be added without touching stored names.

## Consequences

- The shipped OpenBao path convention `trogonai/{owner_id}/credentials/{credential_id}`
  is ratified as-is, with owner id understood as project id, so no stored path
  migrates.
- The project id is what a credential aggregate's resolver scopes its subjects
  by, and therefore what it declares as its
  [ADR#0027](./0027-decider-multi-tenancy-primitive.md) `SubjectScope`. #0027
  owns no tenant value of its own; the project id is the consumer-side vocabulary
  it expects.
- Domain work names the owner value object as a project id rather than
  inventing a parallel workspace concept; workspace-shaped fields collapse
  into the project.
- Cross-project credential moves are disallowed, matching GCP Secret
  Manager; a credential is born in a project and dies there.

## References

- [ADR#0027: Declared Subject Scope for Decider Stream Resolution](./0027-decider-multi-tenancy-primitive.md)
- [ADR#0040: Contract Field Vocabulary: Identifiers, Handles, and Display Labels](./0040-contract-field-vocabulary.md)
- [ADR#0050: Signed Proof-of-Possession as the Strongly Recommended Caller Authentication](./0050-signed-first-caller-authentication.md)
- [ADR#0051: Fully Bound Per-Request Signing Contract](./0051-fully-bound-request-signing.md)
- Google API Improvement Proposals, resource-oriented design (aip.dev)
- Google Cloud Secret Manager resource model and Vercel environment scoping
