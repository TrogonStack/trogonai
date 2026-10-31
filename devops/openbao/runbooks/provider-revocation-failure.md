# Runbook: Provider Revocation Failure

Revoking a credential on the platform stops the platform from using it. It
does not tell the provider (GitHub, Slack, Linear, Sentry, and the rest) to
stop honoring it. For a leaked bot token or API key, provider-side revocation
is the part that actually matters.

**State: provider-side revocation is not implemented.** No source plugin
calls a provider revoke endpoint, and there is no retry queue or failure
signal for one, because there is no attempt to fail. Every provider-side
revocation today is a human operating the provider's console.

**Customer-visible impact:** the integration stops working, which is the
intended effect of a revocation. The risk is the inverse: the customer
believes the credential is dead everywhere, and it is dead only here.

## The procedure today

Platform revocation and provider revocation are two separate acts. Both are
required, and neither implies the other.

**1. Revoke on the platform.** See
[secret-leak-response.md](secret-leak-response.md) for the API call. This is
immediate and it is what bounds the platform's own use of the credential.

**2. Revoke at the provider, manually.** In the provider's own console:
regenerate the webhook signing secret, revoke the bot token, delete the API
key, rotate the OAuth client secret. Which action applies depends on the
credential kind, and the provider's terminology rarely matches ours.

**3. Confirm at the provider.** The credential no longer appears, or appears
as revoked, in the provider's own listing. A console that returned success is
not confirmation; the listing is.

**4. Record which providers were done and which were not.** Partial
provider-side revocation across a multi-integration incident is the normal
outcome under time pressure, and an unrecorded gap is indistinguishable from
a completed one a week later.

## When the provider-side revoke cannot be completed

Sometimes it cannot: the provider account is owned by the customer, not by
us; the console is down; the credential was issued by an identity nobody
still has.

In that case the credential remains live at the provider indefinitely. The
honest response is:

- Tell the customer explicitly that the credential is still valid on their
  side and that only they can revoke it. Give them the exact provider-side
  action.
- Record it as an open item in the incident, not as a completed revocation.
- If the credential grants write access to customer data, the exposure
  continues until they act, and the incident does not close when our side is
  clean.

## What the implementation would need

Recording the shape so it is not rediscovered:

- Provider revocation is per-source and per-kind. There is no generic
  endpoint; each provider has its own API, its own auth requirements for
  performing a revoke, and its own idea of what revoking means. Some have no
  revoke API at all, which means the manual path never fully goes away and
  the design has to model "not supported by this provider" as a first-class
  outcome rather than a failure.
- Revoking a credential often requires a *different* credential with
  administrative scope, which the platform may not hold. The credential being
  revoked usually cannot revoke itself.
- It must be retried and it must be observable. A provider revoke that fails
  silently is worse than one that is never attempted, because the record says
  it happened.
- The event stream needs to distinguish "revoked on the platform" from
  "revoked at the provider." Today it records only the former, and collapsing
  the two would misrepresent every credential revoked before provider support
  existed.
- The failure needs an alert. [alerts.md](alerts.md) lists provider
  revocation failure among the alerts with no signal today, because there is
  no operation to emit one.
