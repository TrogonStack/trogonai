---
number: "0097"
slug: github-app-installation-lifecycle
status: draft
date: 2026-11-05
---

# ADR#0097: A GitHub App Installation Is a Provider Fact Learned by Reconciliation, Bound to an Owner Only by an In-Session Claim, and Never by the Setup Redirect

## Context

[ADR#0092](./0092-third-party-connection-model.md) names the GitHub App as a
`ProviderApp` of kind `KeyedApp { app_id, private_key SecretRef }` and notes
that GitHub Apps are not OAuth clients. [ADR#0094](./0094-oauth-authorization-protocol.md)
then describes how a connection is authorized, and every step of it assumes an
OAuth authorization code: a signed `state`, a callback that is claimed as a
write, a code exchange, and a staged token bundle. GitHub App installation has
none of those parts.

A GitHub App is installed on the provider's site. The person picks an account,
an organization or a user, and a set of repositories. GitHub then records the
installation on its side and emits two signals that the platform may or may
not receive:

- an `installation.created` webhook, signed with the app's webhook secret,
  carrying the installation id, the account, the repository selection and the
  GitHub user who performed the install;
- a browser redirect to the app's configured setup URL, carrying the
  installation id and a `setup_action`.

Neither signal is reliable. A webhook can be dropped, delayed, or arrive before
the platform has restarted. A redirect can be blocked by the browser, lost when
the tab is closed, or never happen at all when the install starts from the
GitHub marketplace. The installation exists on GitHub either way. Whatever the
platform does, it cannot condition the existence of the installation, or the
integrity of the record that describes it, on either signal arriving. Treating
the redirect as the write that links an installation to an owner is the
distributed transaction that [ADR#0094](./0094-oauth-authorization-protocol.md)
spent its write-ahead machinery avoiding, and GitHub offers no `state`
parameter that survives into the webhook to recover from it.

GitHub does offer one authoritative read. With a JSON Web Token signed by the
app's private key, `GET /app/installations` lists every installation of the
app, and `GET /app/installations/{installation_id}` describes one. The platform
can call these at any time, with no browser and no user present. An installation
access token for one installation is minted from the same JWT with
`POST /app/installations/{installation_id}/access_tokens`, lasts one hour, and
has no refresh token.

Today the gateway handles GitHub inbound only. The `github` source verifies
webhook signatures through runtime credentials
(`rsworkspace/crates/platform/trogon-gateway/src/source/github`) and publishes
the event. Nothing on main knows what an installation is, who owns one, or how
to act as one.

This ADR decides what an installation is to the platform, where its facts come
from, how it becomes a `Connection` with an owner, and what the setup redirect
is allowed to do.

## Decision

**An installation is a fact the platform learns from GitHub by reconciliation
and holds as `Unclaimed` until a principal, in their own session, claims it
with proof. The setup redirect writes nothing.**

### 1. GitHub is the only writer of installation facts, and reconciliation is how the platform reads them

The connections service keeps one `GitHubInstallation` record per installation
id, scoped to the `ProviderApp` that owns the private key. Its content is what
GitHub reports: the installation id, the target account with its GitHub numeric
id and type, the repository selection, the granted permissions, suspension, and
the GitHub user recorded as the installer when a webhook supplied one.

The record is written by a reconciler that calls `GET /app/installations` with
the app JWT and upserts by installation id. An installation GitHub no longer
lists is marked `Removed`. The reconciler runs on service start, on a timer,
and whenever any GitHub webhook for the app arrives. Each run is idempotent and
converges on GitHub's view regardless of which signals were lost in between.

The gateway's `github` webhook source stays as it is: it verifies the signature
and publishes the event. `installation`, `installation_repositories` and
`github_app_authorization` events become reconciliation triggers in the
connections service. A dropped webhook costs latency, never correctness.

### 2. Every installation starts Unclaimed, and nothing acts as an Unclaimed installation

A newly reconciled installation is `Unclaimed`. It has no owner, no custody
project and no `Connection`. Custody does not mint an installation token for
it, the proxy of [ADR#0096](./0096-connection-use-by-agents.md) cannot select
it, and no agent declaration resolves to it.

An `Unclaimed` installation stays until GitHub reports it removed or a claim
succeeds. There is no expiry on the platform side: the installation is real on
GitHub, and discarding the platform's knowledge of it would only hide a
standing grant of access from the people who should see it.

### 3. Ownership is written by one in-session command, with proof

A principal binds an installation with `ClaimInstallation { installation_id,
custody_project, owner }`. The command is issued from an authenticated platform
session, never from a provider redirect, and it is the only path from
`Unclaimed` to `Claimed`.

The command is admitted only when the caller proves a relationship to the
installation's account that GitHub can confirm:

- the caller has a linked GitHub identity whose numeric user id equals the
  installer recorded on the installation; or
- `GET /user/installations`, called with the caller's GitHub user token, lists
  the installation, which GitHub returns only for users who can administer it.

Proof is checked at claim time against GitHub, not against a value remembered
from an earlier signal. The caller must also hold the right to create a
connection in the named custody project under
[ADR#0093](./0093-connection-ownership-and-delegation.md).

On success the service takes the `ProviderAccountClaim` of
[ADR#0092](./0092-third-party-connection-model.md) Decision 6, whose fingerprint
already includes the installation, creates the `Connection` pinned to the
GitHub connector release and the `KeyedApp` provider app, and records the
installation as `Claimed(connection_id)`. Two concurrent claims on one
installation resolve through the claim's no-stream precondition: one wins, and
the other fails with the typed `ACCOUNT_ALREADY_CONNECTED` reason that
[ADR#0094](./0094-oauth-authorization-protocol.md) Decision 5 defines.

### 4. The setup redirect is a view route

The GitHub App registers a setup URL per environment, hosted on the same
callback edge as [ADR#0094](./0094-oauth-authorization-protocol.md) Decision 7,
with "redirect on update" enabled. The route reads the installation id from
the query, requires the caller's platform session, and renders the claim page
for that installation. If the reconciler has not yet seen the installation, the
route asks it to fetch that one installation inline and renders whatever GitHub
returns, including "not found yet".

The route performs no write. It carries no signed `state`, because there is
nothing to bind: the installation id is public and the user is identified by
their session. Losing the redirect loses only the convenience of landing on
the right page. The same page is reachable from the project's connections view
for any installation the caller can prove.

A redirect to a setup URL for an installation that a different principal has
already claimed renders that fact and offers nothing else.

### 5. Installation tokens are minted from the private key, never stored long-term

For a `Claimed` installation, custody mints an installation access token on
demand: sign a JWT with the `KeyedApp` private key held under the provider
app's `SecretRef`, call the access-token endpoint for that installation id, and
hold the result in memory with its expiry. The token is re-minted when it is
near expiry or when GitHub rejects it. There is no refresh token and no
write-ahead refresh transaction, because nothing is exchanged: a lost response
is retried, and the previous token simply expires.

The `CredentialBinding` of the `Connection` names the role `installation` and
refers to the provider app's private key `SecretRef`. No token enters any
stream, which keeps [ADR#0092](./0092-third-party-connection-model.md)
Decision 8 unchanged.

Repository selection can be narrowed at mint time with the endpoint's
`repositories` and `permissions` parameters when a grant asks for less than the
installation holds. The ceiling is what GitHub granted the installation.

### 6. Provider-side changes flow through the same reconciliation

`installation.deleted`, a `suspend`, or an installation missing from the listing
marks the record `Removed` or `Suspended`. A `Claimed` installation whose record
becomes `Removed` moves its `Connection` to `Revoked`, terminal under
[ADR#0092](./0092-third-party-connection-model.md) Decision 4, and the account
claim is released. A `Suspended` installation moves its `Connection` to
`Suspended` and back when GitHub lifts it. Repository additions and removals
update the record and are visible to admission without touching the
`Connection`.

Disconnecting from the platform side calls
`DELETE /app/installations/{installation_id}` so the provider's view and the
platform's agree, then the next reconciliation confirms the removal.

## Alternatives Considered

**Bind the installation to the initiating principal through a signed `state`
on the setup redirect.** Rejected. GitHub does not carry `state` into the
webhook, so the binding exists only if the redirect lands. A blocked or lost
redirect leaves a real installation with no owner and no recovery except
exactly the in-session claim this ADR makes the only path. Making the redirect
a write adds a failure mode and removes nothing.

**Treat the `installation.created` webhook as the write that creates the
`Connection`.** Rejected. The webhook names GitHub identities only. It cannot
say which platform project or principal the installation is for, and two
tenants whose members share a GitHub account would be indistinguishable. It is
also not guaranteed to arrive. It is the right trigger and the wrong source of
truth.

**Auto-claim when the installer's GitHub id matches exactly one linked
principal.** Deferred. The match is a sufficient proof under Decision 3, but
the custody project still has to be chosen, and choosing it silently would
place a standing grant of repository access into a project nobody picked. A
future revision may preselect the project on the claim page when the
principal has one; the command stays explicit.

**Model the installation as an `AuthorizationAttempt` with a GitHub profile.**
Rejected. An attempt is single-use, write-ahead and ends in a staged token
bundle. An installation is a durable provider fact that can exist for weeks
before anyone claims it and that GitHub can change at any time. Forcing it
through the attempt states would invent phases the provider does not have.

**Require "request user authorization during installation" so every install
also yields a user token.** Rejected as a requirement. It makes the install
depend on a second redirect, which is the dependency this ADR removes. The
user-authorization flow remains available as an ordinary
[ADR#0094](./0094-oauth-authorization-protocol.md) connection when a feature
needs to act as a person rather than as the installation.

## Non-Goals

- Acting as a GitHub user. That is an OAuth connection under
  [ADR#0094](./0094-oauth-authorization-protocol.md).
- Fine-grained authorization of individual GitHub API calls. The proxy of
  [ADR#0096](./0096-connection-use-by-agents.md) bounds forwarded HTTP; GitHub's
  installation permissions are the ceiling.
- Customer-supplied GitHub Apps. The `ProviderApp` origin
  `CUSTOMER_SUPPLIED` of [ADR#0092](./0092-third-party-connection-model.md) is
  compatible with this ADR but is not enabled by it.
- Processing repository events such as `push` or `pull_request`. The gateway
  keeps publishing them; routing them by installation to a claimed connection
  is a consumer concern.

## Consequences

- An installation nobody claims is visible as `Unclaimed` to anyone who can
  prove a relationship to its account, and to platform operators. It does
  nothing until claimed.
- The connections service gains a reconciler with provider egress to
  `api.github.com` under the app JWT. It is the only component that calls the
  installations listing.
- `ClaimInstallation` is the first connection command whose authorization
  depends on a live provider read. It fails closed when GitHub is unreachable.
- The setup URL and the webhook URL are registered per environment on the
  callback edge, and neither needs a secret beyond the webhook signing secret
  the gateway already holds.
- [ADR#0094](./0094-oauth-authorization-protocol.md) applies to OAuth providers
  and the user-authorization flow of GitHub only; it does not describe GitHub
  App installation. [ADR#0095](./0095-connection-credential-custody-and-refresh.md)
  gains a keyed-app credential path with no stored token set and no refresh
  transaction.

## References

- [ADR#0092](./0092-third-party-connection-model.md): `ProviderApp`,
  `KeyedApp`, `ProviderAccountClaim`, connection states
- [ADR#0093](./0093-connection-ownership-and-delegation.md): custody project
  and owner
- [ADR#0094](./0094-oauth-authorization-protocol.md): the OAuth attempt this
  ADR does not use, and the callback edge it shares
- [ADR#0095](./0095-connection-credential-custody-and-refresh.md): custody and
  refresh for token sets
- [ADR#0096](./0096-connection-use-by-agents.md): proxy use of a connection
- GitHub Docs: Authenticating as a GitHub App installation; Generating an
  installation access token; Webhook events `installation` and
  `installation_repositories`; `GET /user/installations`
