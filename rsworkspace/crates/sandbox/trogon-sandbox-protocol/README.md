# Sandbox protocol

`trogon-sandbox-protocol` holds the behavior that sits on top of the validated types in
`trogon-sandbox-contract`: how subjects and keys are built, how dispatches are signed and
verified, how attempts are recorded, how policy is evaluated, and how secrets are kept out
of guest-visible output. The contract crate owns the data; this crate owns what is done
with it.

## Contents

- Subjects and keys: `subjects`, `kv_key`, `lifecycle_subject`, `SignedSubject`, and the
  error classification used when a reply is built.
- NATS helpers: `nats_connect` builds a connection from a URL, `nats_transfer` moves
  payloads larger than one message in bounded chunks, and `subscribe_verified` delivers
  only messages whose dispatch signature checks out.
- Dispatch authority: `DispatchSigner` and `DispatchVerifier` sign and check ed25519
  dispatch envelopes, a fence file rejects replays and stale epochs, and
  `RedispatchGuard` stops a lease from being dispatched twice.
- Attempt ledger: `AttemptLedger` keeps a SQLite record of every dispatch attempt so
  cleanup survives a worker restart. `AuthorityGuard` pauses and purges leases whose
  authority lapses.
- Policy evaluation: filesystem glob matching, domain and host pattern matching, request
  rule and response header matching, and proxy routing for `NetworkOptions`.
- Secrets: `SentinelRegistry` swaps real credentials for per-host sentinels, the extract and
  JWT maskers rewrite credential files without exposing the secret, and `RedactionSet`
  with `StreamRedactor` scrubs known values from guest output. `cap_buffered` and `recap` bound what a
  command may return.
- Sequencers: stdin and terminal sequencing, entity and environment transitions, phase
  timers, and heartbeats.

## Relationship to the other sandbox crates

Every parsed type comes from `trogon-sandbox-contract`; this crate adds free functions and
helpers over those types and never redefines them. Connection handling reuses
`trogon-nats`.

## Tests

Tests that need a NATS server start one in a container through `trogon-nats`'s
`test-support` feature, so they need Docker.
