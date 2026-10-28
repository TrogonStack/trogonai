---
term: "Dedicated key"
section: "Identity, security, and multi-tenancy"
order: 4
---

# Dedicated key

The opt-in key custody tier between managed key and customer managed key. The
key-encryption key lives in the same platform-operated backend as a managed
key but wraps exactly one tenant's envelopes, one key per tenant per purpose.
The tenant sees the key's identity and usage audit, rotation runs for that
tenant alone, and destroying the key erases that tenant's envelopes without
touching anyone else's. Policy and availability stay platform-controlled. See
[ADR#0033](../adr/0033-key-custody-product-model.md) and
[Key Custody](../architecture/key-custody.md).
