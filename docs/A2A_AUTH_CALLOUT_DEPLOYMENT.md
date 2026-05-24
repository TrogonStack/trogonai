# A2A auth callout — deployment

Operator runbook for `a2a-auth-callout` signing keys and rotation.

## Signing key custody

Select custody with `AUTH_CALLOUT_SIGNING_KEY_SOURCE`:

| Value | Use | Notes |
|-------|-----|--------|
| `env` | Local dev only | Default. Reads `AUTH_CALLOUT_SIGNING_SECRET` (+ optional `AUTH_CALLOUT_SIGNING_SECRET_PREVIOUS`). Logs a one-time warning that env secrets are not for production. |
| `file` | Production / VMs | Requires `AUTH_CALLOUT_SIGNING_KEY_PATH` (current key material). Optional `AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH` for overlap during rotation. Files are raw PEM or shared-secret bytes (no JSON envelope). |
| `vault` | Planned | Not wired yet; process exits if selected. Future work will load keys from an operator vault behind a feature flag. |

Minted User JWTs carry a `kid` claim (and matching JWS header `kid`) set to the key version string (`current` / `previous` for file and env sources). Verifiers should select the decoding key from the source’s `accepted()` list using `kid`, falling back to trial verification only when `kid` is absent.

## Rotation procedure

1. Generate new signing material. Deploy callout with **new** key at `current` and **old** key at `previous` (`AUTH_CALLOUT_SIGNING_KEY_PATH` + `AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH`, or env equivalents).
2. Wait at least one full User JWT TTL (`AUTH_CALLOUT_USER_JWT_TTL_SECS`, default 300s) so outstanding tokens expire.
3. Redeploy with only the new key at `current`; remove `previous` paths / `AUTH_CALLOUT_SIGNING_SECRET_PREVIOUS`.

During overlap, mint always uses `current`; verification accepts both until step 3.

## Related

- Runtime env reference: [`A2A_RUNTIME_ENV.md`](./A2A_RUNTIME_ENV.md)
- Build tracker: [`A2A_AUTH_CALLOUT_TODO.md`](../A2A_AUTH_CALLOUT_TODO.md)
