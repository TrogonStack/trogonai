# OpenBao Credential Policies

The five credential platform roles, written against the path convention
`OpenBaoSecretStore` actually uses.

Nothing consumes these yet. The local compose stack runs OpenBao in `-dev` mode
with a root token, and no service binds to a policy, because the auth method per
service is still an open decision. These files are the path
half of that answer; the identity half is what the auth-method decision adds.

## Path Convention

`OpenBaoSecretStore::credential_path` builds:

```text
trogonai/{owner_id}/credentials/{credential_id}
```

Both variables are percent-encoded by `encode_path_segment` before the URL is
built, but **OpenBao decodes them again before routing, storage, and policy
matching**. That was verified, not assumed: writing to
`.../credentials/openbao%3Atenant-1%3Agithub%2Fprimary%3Awebhook_secret` stores
a key that lists as `openbao:tenant-1:github/` containing
`primary:webhook_secret`. The encoding is therefore transport-level only and
buys no path containment.

Two consequences drive the rules below:

1. **The credential-id portion spans a variable number of segments.** A
   source-scoped id (`openbao:tenant-1:discord:bot_token`) is one; an
   integration-scoped id (`openbao:tenant-1:github/primary:webhook_secret`) is
   two. So the credential position needs the trailing `*` glob. A
   single-segment `+` there matches nothing at all, which fails closed but
   would have made every policy silently useless.
2. **The owner position is still exactly one segment**, so `+` is correct
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
  secrets-service resolve authorization from ADR#0023, and neither exists yet.

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
one token per role, and asserts 19 allow/deny outcomes against the real
percent-encoded paths, including three escape checks (other subtree, `sys/`,
outside `credentials/`). Needs Docker. Run it after editing any `.hcl` in this
directory: the first draft of these policies parsed and uploaded cleanly while
denying every single legitimate request, and only an end-to-end request caught
it.
