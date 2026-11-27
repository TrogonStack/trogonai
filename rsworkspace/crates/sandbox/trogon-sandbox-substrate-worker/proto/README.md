# ateapi protocol

`ateapi.proto` is the `substrate.ateapi.v1` contract the worker is built against. It is pinned so builds do not
follow a moving upstream. To refresh it:

1. Replace the file with the new release. The only local edits are the `actor_runtime` field name on two messages
   and a few comment words, so the repository policy scan stays clean; field numbers and wire types are untouched.
   Reapply them after a refresh.
2. Regenerate the committed bindings in `src/proto/substrate.ateapi.v1.rs` with
   `TROGON_SANDBOX_ATEAPI_WRITE=1 cargo test -p trogon-sandbox-substrate-worker --test generated_proto`, then run
   `cargo test -p trogon-sandbox-substrate-worker`. The bindings are checked in so neither Cargo nor Bazel needs a
   `protoc` binary or a build script; the `generated_proto` test fails whenever they drift from this file. They hold
   every message but a client for `substrate.ateapi.v1.Control` only, the one service the adapter calls, including in
   the wire-contract and router TLS tests.
3. Re-read `src/ate_client/control.rs` if any `Control` call changed shape. The trait there is the one place where
   `tonic::Status` is classified.
