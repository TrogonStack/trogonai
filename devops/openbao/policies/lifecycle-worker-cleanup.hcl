# lifecycle_worker_cleanup
#
# Backs SecretStoreRevoke, SecretStoreDestroy, and the reconciliation jobs.
#
# revoke  -> GET {mount}/metadata/... then POST {mount}/delete/...  (soft, all versions)
# destroy -> POST {mount}/destroy/...                               (permanent, one version)
#
# No `read` on the data endpoint: cleanup decides from metadata and never needs
# plaintext. List capability on the metadata tree is what OpenBao -> DB orphan
# reconciliation walks; without it that job cannot enumerate managed paths.

path "secret/delete/trogonai/+/credentials/*" {
  capabilities = ["update"]
}

path "secret/destroy/trogonai/+/credentials/*" {
  capabilities = ["update"]
}

path "secret/metadata/trogonai/+/credentials/*" {
  capabilities = ["read", "list"]
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

# Undelete is not granted. Reversing a revocation is a break-glass action, not
# something a cleanup worker should be able to do unattended.
path "secret/undelete/trogonai/+/credentials/*" {
  capabilities = ["deny"]
}
