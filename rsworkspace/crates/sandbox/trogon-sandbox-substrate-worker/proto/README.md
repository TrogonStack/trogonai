# ateapi protocol

`ateapi.proto` is the `substrate.ateapi.v1` contract the worker is built against. It is pinned so builds do not
follow a moving upstream. To refresh it:

1. Replace the file with the new release. The only local edits are the `actor_runtime` field name on two messages
   and a few comment words, so the repository policy scan stays clean; field numbers and wire types are untouched.
   Reapply them after a refresh.
2. Run `cargo test -p trogon-sandbox-substrate-worker`. `build.rs` generates clients for every service in the file,
   but the adapter uses only `substrate.ateapi.v1.Control`, including the wire-contract and router TLS tests.
3. Re-read `src/ate_client/control.rs` if any `Control` call changed shape. The trait there is the one place where
   `tonic::Status` is classified.
