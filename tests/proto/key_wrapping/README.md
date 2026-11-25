# Key wrapping protobuf fixtures

These fixtures check the design-only `secrets.exchange` wrapping contracts under
[ADR#0030](../../../docs/adr/0030-customer-controlled-key-backend-routing.md) and
[ADR#0083](../../../docs/adr/0083-sealed-content-envelope.md).

```sh
mise run proto:validate -- --suite key_wrapping
```

`WrapDekRequest` and `UnwrapDekRequest` carry bounded authenticated xkey boxes.
`WrapDekInput`, `UnwrapDekInput`, `UnwrapDekOutput` and `Dek` are inner values
that exist only inside trusted zeroizing memory. No raw DEK belongs in an outer
request/reply, event, retained command, cache, log or trace. Wrapped DEKs persist
only in their data owner's encryption envelope alongside the opaque KeyRef and
binding version.

The exact-key wrap target supports independently authorized migration. Default
selection is explicit; neither mode permits a provider locator or fallback.
Authorization, current admission and actual backend participation precede each
provider attempt. A failure category does not erase earlier uncertain effects.

Version 1 authenticated binding uses the exact length-prefixed encoding stated
in `key_wrapping.proto`; protobuf serialization is not its canonical encoding.
The service derives its tenant/resource/purpose from authoritative context,
compares it with the received fields and binds the original request/reply
context before using any result. Operation/correlation identifiers join the
existing key-custody audit record without recording cryptographic material.
Safe reply evidence separately records publication, withholding or unknown
delivery. A successful provider operation remains successful in its audit even
when live authority subsequently requires withholding its output. Publication
does not prove that a caller received or used a result.

Fixtures prove shape, exact DEK length, closed versions/suites/verbs, finite
original deadline, explicit target and result selection, ciphertext bounds and
typed failures. Test keys, DEKs and ciphertext are public synthetic fixtures.
They do not prove the xkey boxes open, that a sender or tenant is authenticated,
that the binding codec is implemented, that custody checks current policy, that
buffers zeroize or that an adapter enforces cryptographic binding. The existing
NATS/xkey threat model, including its hostile-server limitation, is unchanged.
Runtime parsing, encrypted core transport, codegen promotion, provider conformance
and data-owner CAS migration remain separate implementation gates.
