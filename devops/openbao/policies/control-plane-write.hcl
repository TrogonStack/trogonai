# control_plane_write
#
# Backs SecretStorePut, SecretStoreRotate, and SecretStoreMetadata.
#
# put   -> POST {mount}/data/...     then POST {mount}/metadata/...
# rotate-> GET  {mount}/metadata/... then POST {mount}/data/... and POST {mount}/metadata/...
#
# There is deliberately no `read` on the data endpoint. The write path never
# reads plaintext back, so a leaked control-plane token can overwrite material
# it cannot exfiltrate. Reconciliation compares metadata, not values; if a
# future job needs read-back, that is a separate decision and a separate role.

path "secret/data/trogonai/+/credentials/*" {
  capabilities = ["create", "update"]
}

path "secret/metadata/trogonai/+/credentials/*" {
  capabilities = ["create", "read", "update"]
}
