---
term: "Customer managed key"
section: "Identity, security, and multi-tenancy"
order: 4
---

# Customer managed key

The opt-in key custody tier. The key-encryption key lives in a backend the
customer controls (their AWS KMS key, Google Cloud KMS key, or their own
OpenBao), so the customer keeps an independent operational veto over future
wrap and unwrap operations. See
[ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) and
[ADR#0033](../adr/0033-two-tier-key-custody-product-model.md).
