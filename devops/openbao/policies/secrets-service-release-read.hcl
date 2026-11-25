# secrets_service_release_read
#
# The one fixed policy every per-request child token of the secrets service
# carries, minted through the `secrets_service_release` token role.
#
# get -> GET {mount}/metadata/... then GET {mount}/data/...?version=N
#
# Both endpoints are required: the read checks path metadata first so it can
# fail closed on a revoked or destroyed version. The child is good for exactly
# those two calls, so it cannot be reused for a second credential.

path "secret/data/trogonai/+/credentials/*" {
  capabilities = ["read"]
}

path "secret/metadata/trogonai/+/credentials/*" {
  capabilities = ["read"]
}
