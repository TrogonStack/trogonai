# secrets_service_login
#
# The policy the secrets service's AppRole login token carries. It grants no
# credential access at all: the token can only mint per-request children
# through the `secrets_service_release` token role and keep its own lease
# alive. Every credential read is made by a child carrying
# `secrets_service_release_read`, which the token role is the only way to get.
#
# A leaked login token therefore reads nothing by itself, and every read it
# enables is made by a short-lived, use-bounded child whose metadata names the
# release it served, per ADR#0023 Decisions 3 and 6.

path "auth/token/create/secrets_service_release" {
  capabilities = ["update"]
}

path "auth/token/renew-self" {
  capabilities = ["update"]
}

path "auth/token/lookup-self" {
  capabilities = ["read"]
}
