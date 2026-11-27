# Sandbox auth

`trogon-sandbox-auth` holds the API key store and the authorization check that the
sandbox services share. It builds on the tenant and service account identifiers in
`trogon-sandbox-contract`.

## API keys

A caller presents a `BearerToken`: one or more visible ASCII characters, wrapped so
the value never appears in `Debug` output or error messages. The store never sees
the token; it indexes records by `ApiKeyHash`, the lowercase hexadecimal SHA-256 of
the token (exactly 64 characters).

`ApiKeyRecord` is what a key authenticates as: a `TenantId`, a `ServiceAccountId`, a
label, and a creation time. It serializes as a JSON object with `tenant`,
`service_account`, `name`, and `created_at` in whole seconds since the Unix epoch.
Deserialization enforces the tenant identifier rules, so a stored record that does
not parse is an error and never reads as a missing key.

`ApiKeyStore` resolves a hash to a record or to `None`.

- `NatsKvApiKeyStore` keeps records in a JetStream key-value bucket. `new` opens the
  `sandbox-api-keys` bucket and `new_with_bucket` opens the `ApiKeyBucket` you pick
  (ASCII letters, digits, hyphens, underscores). A bucket that exists keeps its
  configuration; one that does not is created with defaults. A failed existence check
  is reported and never treated as "absent". `mint` generates a 32-byte random token,
  stores its record under the hash, and returns the token once.
- `InMemoryApiKeyStore` holds records in process memory for tests and single-process
  setups.

## Authorization

`Authorizer::authorize_sandbox_lease` decides whether a service account may lease
sandboxes for a tenant, given a `SandboxAuthzRequest`. It returns `AuthzError::Denied`
when the permission is absent and `AuthzError::Backend` when the check could not be
made.

- `NoopAuthorizer` allows every request.
- `SpiceDbAuthorizer` asks a SpiceDB permission service whether the service account
  holds `lease_sandbox` on the tenant. `SpiceDbClient` makes the gRPC call;
  `SpiceDbEndpoint` and `SpiceDbToken` are parsed connection settings. The token is
  wrapped, sent as a sensitive bearer header, and never printed. Tenants are checked
  as the `TenantResourceType` object type, `sandbox_tenant` unless another is chosen.

## Fit

The broker, gateways, and admin tooling build on `ApiKeyStore` and `Authorizer`
without depending on how keys are stored or where permissions live.
