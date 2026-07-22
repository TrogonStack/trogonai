# a2a-auth-callout

NATS auth-callout server for the A2A protocol. Verifies caller credentials
(mTLS, OIDC, API key) and mints scoped NATS user JWTs that bind connections
to a tenant account and to the subject patterns the caller is allowed to
publish/subscribe on.

This crate is being landed in slices on `main`; see
`.trogonai/todos/a2a-auth-callout-pr-plan.internal.trogonai.md`.
