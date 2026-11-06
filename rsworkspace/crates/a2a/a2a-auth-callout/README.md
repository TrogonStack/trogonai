# a2a-auth-callout

NATS auth-callout server for the A2A protocol. Verifies caller credentials
(mTLS, OIDC, API key) and mints scoped NATS user JWTs that bind connections
to a tenant account and to the subject patterns the caller is allowed to
publish/subscribe on.

Every minted token names its caller in two claims: `caller_id` and
`principal_kind` (`agent`, `person` or `service`). Together they form the
`{kind}.{id}` principal position of the A2A gateway ingress, agent delivery
and task event subjects, which is how the bridge and the client adapters learn
the principal to publish under. Until credentials assert their own kind, OIDC
callers mint as `person` and mTLS and API key callers as `service`.

A process that only checks minted tokens, such as the A2A gateway, holds an
`IssuerPublicKeys` (the issuer account public keys, current plus previous
during a rotation) and never a signing seed. `UserJwtClaims::verify_minted_user_jwt`
takes any `VerifyingKeySource`, which every `SigningKeySource` is as well, so
the callout verifies against the keys it mints with.

The binary answers two requests on one connection. The server's
`$SYS.REQ.USER.AUTH` requests admit direct callers, and the JSON mint requests
of an A2A bridge arrive on `a2a.bridge.auth.callout.request`, or on
`AUTH_CALLOUT_MINT_SUBJECT` when set. Both run through the same dispatcher and
verifiers, and both subscribe in the `AUTH_CALLOUT_QUEUE_GROUP` queue group, so
replicas share the load. A mint request must carry a sealed credential. A
request that is malformed, unsealed or refused gets a reply naming only a
refusal category, never a JWT.

## Tenant admission authority

Credential verification does not grant tenant membership. Every dispatcher
requires a `TenantAdmission` authority, and both direct and bridge mint requests
must receive an explicit permit for the exact tenant account and verified
principal. A configured tenant signing key or tenant-scoped caller hash does not
provide that permit. Failed admission cannot select another credential scheme.

The binary requires `AUTH_CALLOUT_TENANT_MEMBERSHIPS` to name a trusted local JSON
registry. It validates the registry before connecting and rereads it for every
mint. Missing, invalid, oversized or unavailable registry state refuses admission
without reusing an earlier decision. An empty membership list denies all callers.
The registry is limited to 1 MiB and 4096 exact bindings. Reads have a one-second
deadline and at most 16 workers; saturation refuses new admissions.

```json
{
  "version": 1,
  "memberships": [
    {
      "account": "<tenant account public NKey>",
      "identity": {
        "type": "oidc",
        "issuer": "https://issuer.example",
        "subject": "<administratively approved OIDC subject>"
      }
    }
  ]
}
```

Provision the file through the administrative deployment boundary, inaccessible
to callers and workload guests. The tagged `identity` selects an exact source:
`oidc` requires `issuer` and `subject`; `legacy_mtls` and `legacy_api_key` require
an administratively approved `subject`. OIDC issuer strings match exactly and
must use HTTPS. No selector interprets wildcards or accepts an implicit source.

For an existing verified identity, `identity.type=principal` requires
`principal_kind` and canonical `principal_id`. Obtain those fields with
`TenantMembership::from_verified_credential` after the configured verifier has
authenticated the credential: use the binding's `principal().kind()` and
`principal().id()`. Do not copy caller-provided principal IDs, token claims or
unverified certificate subjects. The tenant is an explicit administrative membership decision, even
when the credential is valid. Never write credentials or private keys to this
registry or logs.

Use an atomic file replacement to apply membership changes. Removal affects the
next mint; already issued broker tokens retain their bounded validity and require
the enterprise session-revocation integration for immediate termination. The
legacy mTLS and API-key subject helpers reproduce their existing identity
namespaces and do not supply SPIFFE attestation. Protect legacy certificate
issuance so a subject cannot be reassigned across accepted issuers.

## OIDC issuer and discovery profile

Issuer configuration requires HTTPS without URL credentials, query or fragment.
The configured issuer, discovery document and signed token must match exactly,
including a trailing slash. Discovery endpoint construction does not change that
identity. JWKS URLs must stay on the issuer origin and also exclude credentials,
query and fragment; redirects are refused.

Discovery and JWKS responses must have successful HTTP status and fit within
256 KiB, including responses without a content length. Connection and complete
request deadlines bound remote work. Static test fixtures do not weaken the
production HTTPS requirement.

## Certificate verification and admission lifetime

CA trust uses standard certificate-path validation with explicit signing-key
usage checks. A configured end-entity pin trusts exactly that certificate's DER,
not another certificate with the same subject or public key. Pins still enforce
leaf validity, purpose and critical-extension checks. Legacy certificates may
omit key/extended-key usage; a present incompatible usage is refused.

Present a leaf-first certificate chain containing the required intermediates.
The callout preserves one ordered chain and never combines alternate verified
paths. Presented and verified alternatives must name the same leaf identity.

Derived NATS credentials are bounded by verified OIDC or selected certificate-path
expiry as well as the configured broker TTL. Expiry is checked again immediately
before minting. OIDC not-before dates are enforced without early-activation
leeway; malformed dates and unsupported critical JOSE extensions are refused.
This profile requires representable integer Unix-second dates.
Every broker mint requires at least one whole second of authorized lifetime.
Zero or subsecond maxima are refused, including legacy API-key and direct mint
calls; positive fractional maxima are rounded down.
Finite verification evidence is separate from serialized identity
claims. Deprecated API-key admission remains an explicit compatibility branch,
not a fallback after another verifier refuses a credential.

DN-based mTLS identity is separate from the proposed
[enterprise workload identity profile](../../../../docs/reference/enterprise-workload-identity.md).
These verification rules do not implement SPIRE attestation or credential rotation.
