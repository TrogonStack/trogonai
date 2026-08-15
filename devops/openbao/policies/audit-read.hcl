# audit_read
#
# Existence, version history, and custom metadata. Never values.
#
# The explicit denies are the point of this file. Omitting a grant is enough
# today, but deny wins over any grant in OpenBao policy evaluation and over any
# other policy attached to the same token, so the plan's "audit roles cannot
# read raw secret values" criterion survives a later policy that is sloppier
# than this one.

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

path "secret/data/trogonai/+/credentials/*" {
  capabilities = ["deny"]
}

# subkeys returns the key structure without values. It still leaks shape, and
# audit has no use for it.
path "secret/subkeys/trogonai/+/credentials/*" {
  capabilities = ["deny"]
}
