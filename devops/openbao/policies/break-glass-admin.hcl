# break_glass_admin
#
# Full control of the managed credential subtree, including undelete, for
# incident recovery. Issued as a short-TTL token behind an approval, never
# attached to a running service identity.
#
# Scoped to trogonai/ on purpose. It grants nothing on sys/, auth/, or any
# other mount, so holding it cannot rewrite policy, create auth roles, or mint
# further access. Escalating past this subtree is a separate, louder step.
#
# Every path here is audit-relevant by definition; the plan's "break-glass
# access is audited" criterion depends on the audit device being enabled on the
# server, which this file cannot assert.

path "secret/data/trogonai/+/credentials/*" {
  capabilities = ["create", "read", "update", "delete"]
}

path "secret/metadata/trogonai/+/credentials/*" {
  capabilities = ["create", "read", "update", "delete", "list"]
}

path "secret/delete/trogonai/+/credentials/*" {
  capabilities = ["update"]
}

path "secret/undelete/trogonai/+/credentials/*" {
  capabilities = ["update"]
}

path "secret/destroy/trogonai/+/credentials/*" {
  capabilities = ["update"]
}

path "secret/metadata/trogonai/" {
  capabilities = ["list"]
}

path "secret/metadata/trogonai/+/" {
  capabilities = ["list"]
}

path "secret/metadata/trogonai/+/credentials/" {
  capabilities = ["list"]
}
