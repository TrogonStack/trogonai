---
term: "Managed key"
section: "Identity, security, and multi-tenancy"
order: 3
---

# Managed key

The default key custody tier. The key-encryption key is a platform-owned key,
one per purpose, shared by every tenant that has not selected another tier and
operated in the platform's registered backend (OpenBao Transit by default).
A tenant configures nothing; tenants are kept apart by the authenticated
binding on every wrap and unwrap, not by separate keys. See
[ADR#0033](../adr/0033-key-custody-product-model.md) and
[Key Custody](../architecture/key-custody.md).
