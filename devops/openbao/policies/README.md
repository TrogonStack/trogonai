# OpenBao Credential Policies

The credential platform roles, written against the path convention
`OpenBaoSecretStore` actually uses.

The local compose stack runs OpenBao in `-dev` mode with a root token. The
secrets service's release path is the one consumer bound to an identity so far:
it logs in with an AppRole and reads through per-request child tokens, as
described under [Secrets Service Release Reads](#secrets-service-release-reads).
The other roles still wait on the auth-method decision for their identity half.

## Path Convention

`OpenBaoSecretStore::credential_path` builds:

```text
trogonai/{owner_id}/credentials/{credential_id}
```

Both variables are percent-encoded by `encode_path_segment` before the URL is
built, but **OpenBao decodes them again before routing, storage, and policy
matching**. That was verified, not assumed: writing to
`.../credentials/a%3Ab%2Fc` stores a key that lists as `a:b/` containing `c`.
The encoding is therefore transport-level only and buys no path containment.

Two consequences drive the rules below:

1. **The credential position is one segment.** The credential id is a
   canonical UUID, which never contains a `/`. The policies glob that position
   with a trailing `*`, which matches the one segment.
2. **The owner position is also exactly one segment**, so `+` is correct
   there and owner subtrees cannot be crossed. That holds because
   `CredentialOwnerId` admits only alphanumerics, `-`, `_`, `.`, and `:`, and
   `SourceIntegrationId` rejects path separators. Containment comes from those
   validators, not from the encoding. Phase 5's "external callers must never
   provide arbitrary OpenBao paths" rests on the same validators.

Dot segments do not traverse: an owner position of `..` is answered with a 301
by the router rather than resolving upward.

The mount is configurable (`OpenBaoMount`, default `secret`). These files
hardcode `secret/`; change the prefix if the deployment mounts elsewhere.

## Endpoints Per Store Trait

The role split follows the segregated store traits, which is why extraction to
the secrets service does not reshuffle these policies.

| Trait                  | OpenBao calls                                        | Role                       |
| ---------------------- | ---------------------------------------------------- | -------------------------- |
| `SecretStorePut`       | `POST data`, `POST metadata`                          | `control_plane_write`      |
| `SecretStoreRotate`    | `GET metadata`, `POST data`, `POST metadata`          | `control_plane_write`      |
| `SecretStoreGet`       | `GET metadata`, `GET data?version=N`                  | `gateway_read`             |
| `SecretStoreMetadata`  | `GET metadata`                                        | all reading roles          |
| `SecretStoreRevoke`    | `GET metadata`, `POST delete`                         | `lifecycle_worker_cleanup` |
| `SecretStoreDestroy`   | `POST destroy`                                        | `lifecycle_worker_cleanup` |

`get` and `rotate` both read metadata before touching data, so a read grant on
`metadata/` is load-bearing for those roles rather than a convenience.

## Acceptance Criteria Status

Against the authorization matrix in CREDENTIAL_PLATFORM_SPEC.md:

Each claim below is asserted by `verify.sh`, not argued from the file contents.

- **Control plane can write credential material.** Covered, and it cannot read
  values back.
- **Cleanup worker can revoke or destroy only scoped paths.** Covered; the
  wildcards cannot escape `trogonai/+/credentials/`, and undelete is denied so
  reversing a revocation stays a break-glass action.
- **Audit roles cannot read raw secret values.** Covered, and denied explicitly
  rather than merely ungranted.
- **OpenBao paths are deterministic and reconcilable.** Covered by the
  convention above plus list capability for the orphan-scan direction.
- **Gateway can read only active refs it is authorized to resolve.** *Partly.*
  A path policy scopes *which* credentials a role can reach; it cannot express
  *active*, because activeness lives in the aggregate, not the path. Fail-closed
  on revoked and destroyed versions comes from `SecretStoreGet::get` checking
  metadata status before reading data. The "authorized to resolve" half needs
  either per-owner identity templating (commented in `gateway-read.hcl`) or the
  secrets-service resolve authorization from [ADR#0023](../../../docs/adr/0023-secret-management-and-key-custody-direction.md), and neither exists yet.

## Secrets Service Release Reads

`OpenBaoReleaseReader` never reads a credential with its login token. The login
token carries `secrets_service_login`, which grants no credential path at all:
only minting through the `secrets_service_release` token role, `renew-self`, and
`lookup-self`. Each release read mints its own child through that role, carrying
`secrets_service_release_read` alone, good for the metadata read and the data
read it makes, and stamped with the release's correlation id in its metadata.
The child is not an orphan, so revoking the login token severs every child it
minted.

The setup the reader expects, with `bao` pointed at the server:

```sh
bao auth enable approle

bao write auth/token/roles/secrets_service_release \
  allowed_policies=secrets_service_release_read \
  orphan=false renewable=false token_no_default_policy=true \
  token_explicit_max_ttl=30s

bao write auth/approle/role/secrets-service \
  token_policies=secrets_service_login token_no_default_policy=true \
  token_ttl=60s token_max_ttl=120s

bao write sys/config/auditing/request-headers/x-trogon-correlation-id hmac=false
```

The last line makes OpenBao write the `x-trogon-correlation-id` header each
release call sends into its audit log in plaintext. The child token's metadata
carries the same id under `auth.metadata.correlation_id` whether or not the
header is registered. Audit devices themselves are declared in the server
configuration; OpenBao 2.5 no longer enables them through the API.

The secret id reaches the service as a file it reads at startup, so no standing
OpenBao credential lives in a config file, per
[ADR#0023](../../../docs/adr/0023-secret-management-and-key-custody-direction.md).

The `OpenBaoReleaseReader` container tests in `trogon-secret-store` load both
`secrets-service-*.hcl` files as written here and assert that the login token
reads nothing, that a child reads only under `trogonai/+/credentials/` and only
twice, and that the audit entry for a read carries the correlation id.

## Applying

```sh
for policy in *.hcl; do
  name="${policy%.hcl}"
  bao policy write "${name//-/_}" "$policy"
done
```

Filenames are hyphenated; the policy names themselves are underscored. The
substitution above is what reconciles the two.

## Verifying

```sh
./verify.sh
```

Starts a throwaway dev OpenBao on port 18201, applies every policy here, mints
one token per role, and asserts the allow/deny outcomes against the real
percent-encoded paths, including escape checks (other subtree, `sys/`, and
outside `credentials/` for both the control plane and the cleanup worker).
Needs Docker. Run it after editing any `.hcl` in this
directory: the first draft of these policies parsed and uploaded cleanly while
denying every single legitimate request, and only an end-to-end request caught
it.
