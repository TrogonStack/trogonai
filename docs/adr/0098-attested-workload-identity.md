---
number: "0098"
slug: attested-workload-identity
status: draft
date: 2026-11-06
---

# ADR#0098: Workload Identity Requires Attested Issuance, Rotating X.509 Credentials and Independent Authorization

> **Related proposals:** [Workload identity and portable trust](../architecture/workload-identity.md)
> explains the foundation. The [federation proposal](../research/workload-identity/federation.md)
> and [adapter roadmap](../research/workload-identity/adapter-roadmap.md) describe
> explicit foreign-domain trust, optional environment/provider profiles and
> custody decisions still needed. They do not qualify another issuer, change
> production requirements or advance this ADR's draft status.

## Context

Cryptographic identity, runtime attestation and authorization answer different
questions. A private key proves possession. A workload attestor establishes
which runtime is using that key. Policy decides what that runtime may do.
Neither a `spiffe://` URI nor a successful TLS handshake establishes all of them.

[ADR#0036](./0036-agent-self-certifying-identity.md) anchors application identity
to a root key. [ADR#0037](./0037-agent-identity-governance.md) separates identity
verification from live authority. [ADR#0053](./0053-external-oidc-federation-surface.md)
describes external workload federation and explicitly defers runtime attestation.
The connection drafts separately govern provider consent, custody and use.

The egress proxy verifies a certificate and evaluates destination policy
separately. Its compatibility profile loads TLS files at startup. Its explicit
Workload API profile consumes rotating SVIDs and governed registrations, gates
activation on durable audit and withdraws captured material on source changes.
It supports either explicit static policy compatibility or an authenticated
scoped NATS KV reader. The live reader
replaces complete policy, binds permit audit to its generation and applies
producer deadlines to existing tunnels. Its publisher, independent recovery
fence and operational revocation acceptance remain separate requirements.
The compatibility profile's provisional SPIFFE-shaped alias does not supply
attestation or automatic rotation. The NATS callout's existing
mTLS identity is a certificate DN or fingerprint, not a SPIFFE identity.
These foundations must not be advertised as the workload identity profile below.

## Decision

### Attestation and issuance precede production acceptance

The workload identity profile uses SPIRE's node and workload attestation, registration
and Workload API rather than a new platform CA built from agent-signed CSRs.
Application identity remains the AgentId of ADR0036. An authorized, versioned
registration maps an attested transport identity to its exact tenant, project,
workload role and, where applicable, AgentId. A certificate path or an AgentId
supplied by its requester cannot create that mapping.

Production registrations use verified deployment selectors and a constrained
parent identity. Registering workloads and changing selectors are privileged,
audited control-plane operations. A tenant workload cannot administer the
issuer, register another workload or select another tenant's identity. Broad
selectors that let a caller choose its own identity are refused.
Changes that widen selectors, add trust domains or replace signing authority
require an independently authorized approver and recorded change provenance.
Emergency revocation can take effect immediately; review cannot delay denial.

Node bootstrap uses the deployment's verified identity, such as Kubernetes
projected service-account token attestation with configured cluster trust, or
the selected cloud or hardware attestor. Shared join tokens and a copied agent
signing key are not production attestation. Node admission alone is not workload
admission: the local agent also verifies the calling workload. An unsupported
runtime remains ineligible for this profile until that boundary is implemented
and demonstrated.

Attestation and authorized registration are prerequisites for first issuance
and renewal, not an optional phase after an issuer starts serving. The deployment
must bind the attested runtime, requested identity and issued key; verify
freshness and replay defenses supported by its chosen attestor; and refuse a
missing, mismatched or unavailable attestation. It must not substitute a static
key or a different identity after that refusal.

A custom issuer is a separately reviewed exception requiring equivalent
attestation, constrained registration, key custody, rotation and failure evidence.
Key possession plus a live AgentId status does not meet that exception.

### The identity control plane has its own protected bootstrap

Trust domains and roots are distinct for production and non-production.
Cross-domain federation requires an explicit domain-to-bundle relationship;
a trusted CA for one domain cannot silently assert identities in another.

The SPIRE Server uses an HSM-backed signing key through a supported key manager
and deployment IAM with no stored cloud access key. The deployment records the
provider, key policy, recovery and compromise procedure. A KMS service name or
an approved algorithm is not proof that a particular build or deployment is
FIPS validated. Such claims require evidence for the actual module and mode.

HA servers, a durable datastore, independently provisioned bootstrap trust and
tested backup/restore are required. Registration administration is isolated from
the workload API. Least-privilege service roles, restricted network paths and
durable audit protect both. Restoring an old registration database or trust
bundle cannot silently restore revoked workload access.

[ADR#0023](./0023-secret-management-and-key-custody-direction.md) still keeps
OpenBao behind the secrets service. Selecting SPIRE does not authorize direct
workload reads from OpenBao. The externally provisioned platform NATS credentials
of [ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md) remain a
distinct bootstrap boundary until an equivalent admission path replaces them;
they are not described as secretless merely because SVIDs exist.

### Workload keys and rotating trust stay outside untrusted agent code

Prefer X.509-SVIDs and mutual TLS for internal workload identity. Use a supported
Workload API client that consumes credential and bundle updates. The supervisor
or trusted adapter owns that client and its private keys. Untrusted agent code
receives neither the Workload API socket nor an identity-signing key. Process
or VM isolation must prevent another tenant or workload from using that client.
SPIFFE assumes credential isolation; it does not create it.

The deployment sets a short SVID lifetime and renews before expiry. The initial
platform ceiling is one hour; an operator may choose a shorter lifetime. This
is a product bound, not a SPIFFE requirement. Failure to renew never extends the
credential or permits use after expiry. Key and bundle replacement are atomic
for new connections, with a bounded overlap that allows valid chains during
rotation and retires compromised roots according to the incident procedure.

The trusted supervisor validates each candidate SVID and its authorized
registration before activating it for platform traffic. It durably records a
credential-activation fact containing public fingerprints, registration and
trust revisions, identity scope and validity. Only after acknowledgement and
a final live-generation check may it atomically enable that candidate for new
connections. Revocation racing the audit append must win over activation.
Monotonic registration, bundle and activation generations plus restore fencing
prevent an old acknowledgement from resurrecting authority. The candidate queue
is bounded; failed audit or validation never activates a candidate or extends
a previously active credential.

This gate is required platform integration, not a feature attributed to SPIRE's
ordinary logging. SPIRE can sign and renew credentials internally; its native
issuer events need separate durable collection and monitoring. Ordinary SPIRE
audit output does not prove synchronous sink acknowledgement before CA signing.

Workload API responses are complete snapshots. Omitted identities and bundles
are removals, and `PermissionDenied` withdraws the corresponding identity/trust
set. Apply those removals immediately, independently of audit availability;
never merge them with retained stale authority or select a different default
identity silently. Rotation overlap cannot preserve an explicitly removed or
compromised root. Reconnection must not reactivate an earlier snapshot.

Recheck derived admission credentials at their mint boundary. A broker user JWT
obtained from OIDC or mTLS must not outlive the verified source token or selected
certificate path, even if the configured broker TTL is longer. A source that
expires during verification or dispatch cannot mint new admission authority.
Keep finite verification evidence outside serialized identity claims. Legacy
API-key admission is an explicit compatibility branch, not an assumed expiry or
a fallback after a failed OIDC or certificate check.
Honor signed not-before dates as well as expiry. Present malformed date claims
and unsupported critical JOSE header extensions are refused. Broker OIDC source
validation uses no early-activation leeway; AAuth's explicit bounded clock policy
is evaluated against its injected clock, not a second implicit system clock.
These implementations accept integer Unix-second dates in their supported
timestamp range. Fractional dates are an unsupported profile encoding, not a
claim that every JWT standard prohibits fractional NumericDate values.

OIDC discovery uses authenticated HTTPS and exact issuer identity. A trailing
slash is part of that identity; endpoint construction cannot normalize the
configured issuer or a token's issuer claim. Discovery and JWKS responses require
successful HTTP status and bounded bodies, including chunked responses. Redirects
are refused, and JWKS locations stay on the issuer's origin without URL
credentials, query or fragment.

Validate the certificate path using the standard TLS verifier, including CA,
usage, path-length, validity and critical-extension constraints. SPIFFE-aware
verification additionally enforces the SVID leaf profile, a single URI SAN,
exact trust domain and the authorized registration mapping. Legacy DN-based
mTLS remains a separate compatibility profile; it cannot silently become a
SPIFFE identity or inherit SPIFFE registration authority.

### Identity never substitutes for live permission or delegation

After authentication, authorize the exact operation, tenant, destination,
Connection and execution grant. The receiving service independently derives
the caller from its authenticated transport and registration. Caller-supplied
headers, CSRs, identifiers and public keys are not alternate trust sources.

Before minting a tenant's broker credential, account admission must check an
authoritative binding between the authenticated caller and that tenant. A
configured signing key only proves the callout can sign for the account; a
tenant-scoped caller hash prevents identity collisions but proves no membership.
A production admission path without that binding refuses the mint. Unknown
callers cannot obtain account access merely by naming an existing account.

Federation under ADR0053 authenticates a workload client. It does not establish
user consent. On-behalf-of use must also have a live person delegation and the
applicable policy and consent bounds for that resource; record both the actor
and delegating principal. Orthogonal downstream resources do not share a scope
namespace, so call chaining does not impose a string-set intersection between
unrelated providers' scopes.
An allow-all policy or a valid upstream token alone does not establish user consent.
The AAuth Access Server is not an OAuth authorization server or an RFC8693 token
exchange endpoint.

Recheck time-dependent authority after a deferred interaction and immediately
before minting or dispatch. An expired resource challenge cannot authorize a
later exchange. Its expiry bounds exchange eligibility, not the lifetime of a
new authorization token. AAuth authorization tokens cannot outlive the agent
token used to obtain them; bound subagents and upstream delegated authority
also bound issued authority. Policy runs again on resume. A parked verified
request is not a permanent authorization decision.

### External OAuth follows an explicit provider trust and protection profile

Use workload federation or asymmetric client authentication when the provider
supports it. Provider trust configuration still binds the exact issuer, subject,
audience, client registration and permitted role or resource. An internal broker
cannot mint an arbitrary SaaS provider's access token without that provider's
agreement. Provider-defined refresh tokens and app credentials remain in custody
under [ADR#0095](./0095-connection-credential-custody-and-refresh.md).

Prefer sender-constrained access tokens where the provider supports RFC8705 or
RFC9449 and the trusted caller can satisfy that protocol end to end. mTLS
client authentication does not by itself bind an access token to the client.
Certificate-bound tokens require the actual TLS certificate's `x5t#S256` at the
verifier; AAuth `cnf.jwk` and DPoP are distinct proof protocols. Unsupported
bindings are refused, not treated as bearer tokens.

A provider that only supports bearer credentials needs an explicit reviewed
compatibility profile. Its token remains replayable if stolen; TLS, short local
cache validity, protected custody and restricted egress reduce exposure without
claiming sender constraint or changing the provider's actual token lifetime.
JWT-SVIDs likewise remain bearer credentials unless a separately specified
protocol constrains their use. They are not the internal default.

### Cache only the authorized resource token, never the exchange assertion

Mint a federation assertion for one audience when an exchange is about to
happen. Never persist, cache, log, trace or expose it to model context.

Resource-token cache identity includes the verified tenant and project, exact
Connection or federation binding, provider account and app, issuer, audience,
granted scope, authorization and credential generations, and holder key where
the token is sender-constrained. A display name or AgentId plus audience and
scope cannot identify interchangeable credentials. JWKS and bundle caches are
separate trust-material caches keyed by their verified issuer or trust domain.

Every use still checks live grant and policy authority. Local usability ends
at the earliest applicable provider expiry, local credential bound, grant
expiry, holder-key retirement and authority invalidation. Restarts, reconnects
and restored snapshots cannot erase invalidation generations or resurrect a
cached token. Do not fabricate a provider expiry when it is unknown.

### Rotation and revocation cover connections that already exist

Authenticate and authorize new requests independently of a connection's earlier
handshake. A long-lived tunnel or session needs bounded authority reevaluation
and termination on applicable expiry, revocation or stale policy. Certificate
expiry at handshake alone cannot close an already established TLS connection.

[ADR#0049](./0049-revocation-latency-target.md)'s five-second p99 remains a normal
operation target, not a hard guarantee. The workload identity implementation must also
prove its configured hard staleness bound, no greater than that ADR's 330-second
backstop. Lost invalidations, unhealthy projections and offline hosts cannot
extend authority indefinitely. External tokens already issued retain their own
expiry/revocation behavior under ADR0053; local denial does not revoke them at
the provider.

### Enablement requires evidence for the complete boundary

The [workload identity reference](../reference/workload-identity.md)
defines implementation and acceptance checklists per capability. A deployment
must not enable or advertise this profile while a required capability is absent.
It fails closed rather than inheriting compatibility defaults. Tests of URI
parsing, signatures or a reference state machine do not establish attestation,
rotation, isolation or production acceptance.

Required platform audit records cover registration, credential activation,
denial, policy and grant decisions, accepted bundle changes and administrative
recovery. The activation gate binds received issuance/renewal updates to platform
use; native CA signing events are a separate issuer audit stream. Required
platform audit failure blocks new activation, authority or dispatch. Denial,
revocation, redaction and expiry remain effective if recording fails. Retain
bounded identifiers and reason codes, never raw attestation,
tokens, signing keys, provider bodies or credentials. Export availability and
retention follow their own governed contracts.

## Consequences

- Workload attestation moves before production issuance, closing the stolen-key
  enrollment gap in an issuer based only on agent proof of possession.
- SPIRE is an operational dependency with attestor, registration, key-manager,
  datastore and trust-bundle responsibilities. Supported versions and deployment
  artifacts must be pinned and verified before enablement.
  Verification includes the release's signatures/provenance under configured
  trust, not only a mutable tag or an unauthenticated digest lookup.
- Existing application identity, NATS admission, AAuth and provider custody
  remain distinct authorities. This decision does not invent an OAuth protocol
  binding or remove provider secrets unilaterally.
- The existing static egress slice remains useful but does not satisfy the
  workload identity lifecycle profile. The optional live KV reader adds policy
  lifecycle controls; it does not supply rotating SVIDs, a governed publisher or
  deployment acceptance. These remain required before claiming that profile.
- This ADR remains draft under [ADR#0000](./0000-adr-process.md). It refines the
  target workload identity profile without marking deferred infrastructure implemented
  or changing another ADR's approval status.

## References

- [SPIFFE X.509-SVID specification](https://github.com/spiffe/spiffe/blob/main/standards/X509-SVID.md)
- [SPIFFE JWT-SVID specification](https://github.com/spiffe/spiffe/blob/main/standards/JWT-SVID.md)
- [SPIFFE Workload API](https://github.com/spiffe/spiffe/blob/main/standards/SPIFFE_Workload_API.md)
- [SPIRE workload registration](https://spiffe.io/docs/latest/deploying/registering/)
- [SPIRE Server deployment](https://spiffe.io/docs/latest/deploying/spire_server/)
- [SPIRE AWS KMS key manager](https://github.com/spiffe/spire/blob/main/doc/plugin_server_keymanager_aws_kms.md)
- [RFC8705: OAuth mutual TLS and certificate-bound access tokens](https://www.rfc-editor.org/rfc/rfc8705)
- [RFC9449: OAuth DPoP](https://www.rfc-editor.org/rfc/rfc9449)
- [Pinned AAuth draft](https://github.com/dickhardt/AAuth/blob/90089f80eaccccbd22e32e06946e2aa08f7d67fe/draft-hardt-oauth-aauth-protocol.md#re-authorization)
