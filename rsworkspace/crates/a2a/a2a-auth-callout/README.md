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

This crate is being landed in slices on `main`; see
`.trogonai/todos/a2a-auth-callout-pr-plan.internal.trogonai.md`.
