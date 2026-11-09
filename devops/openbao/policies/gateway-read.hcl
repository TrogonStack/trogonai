# gateway_read
#
# Backs SecretStoreGet and SecretStoreMetadata on the runtime resolve path.
#
# get -> GET {mount}/metadata/... then GET {mount}/data/...?version=N
#
# Both endpoints are required: `get` reads path metadata first so it can fail
# closed on a revoked or destroyed version rather than returning material the
# lifecycle already retired.
#
# Named for the role, not the process. Under ADR#0023 this policy attaches to
# the platform secrets service, which is the only process holding an OpenBao
# client; trogon-gateway holds refs and resolves them over NATS. Until that
# extraction happens the gateway carries this role itself.

path "secret/data/trogonai/+/credentials/*" {
  capabilities = ["read"]
}

path "secret/metadata/trogonai/+/credentials/*" {
  capabilities = ["read"]
}

# Owner-scoped variant, for a resolver identity that serves a single owner
# rather than the whole platform. Requires the auth method to stamp owner_id
# into entity metadata, which is part of the still-open auth-method decision.
#
# path "secret/data/trogonai/{{identity.entity.metadata.owner_id}}/credentials/*" {
#   capabilities = ["read"]
# }
