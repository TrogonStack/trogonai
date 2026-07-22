---
term: "Managed key"
section: "Identity, security, and multi-tenancy"
order: 3
---

# Managed key

The default key custody tier. The key-encryption key is an OpenBao Transit key
operated by the platform behind the secrets service, and a tenant configures
nothing. See [ADR#0033](../adr/0033-two-tier-key-custody-product-model.md) and
[Key Custody](../architecture/key-custody.md).
