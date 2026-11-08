---
term: "CredentialBinding"
section: "Agent execution model"
order: 11
---

# CredentialBinding

The record authorizing the model access service to authenticate one
ModelProviderConnection, holding exactly one authentication method and, where
needed, a stable `SecretRef` rather than raw credentials. See
[ADR#0032](../adr/0032-model-route-and-credential-binding.md).


In the connector plane, package-qualified `connections/types.CredentialBinding`
names one role and opaque SecretRef on a [Connection](./connection.md). It is
an association to custody metadata, not authority by itself, and differs from
the model-plane type above. See
[ADR0092](../adr/0092-third-party-connection-model.md).
