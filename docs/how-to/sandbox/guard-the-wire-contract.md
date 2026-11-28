# Change a public wire type safely

The wire types that brokers, gateways, workers and clients exchange live in
`rsworkspace/crates/sandbox/trogon-sandbox-contract` (request, response and
capability types) and `rsworkspace/crates/sandbox/trogon-sandbox-protocol`
(subjects, dispatch authentication and shared helpers). Peers at different
versions run side by side during a rollout, so a change to the JSON of a wire
type must stay readable by the old peers.

## Make an additive change

Add the field with `#[serde(default)]` and, for optional data,
`skip_serializing_if`. Old peers ignore the new field and new peers accept
messages that lack it. Do this for every change you can.

## Change the contract on purpose

1. Decide the rollout first. A breaking shape, such as a renamed field, a
   removed variant or a changed tag, needs a version or a flag and a window
   where both shapes are accepted.
2. Update every producer and consumer of the type in the same change: the
   broker, the gateways, the workers, `trogon-sandbox-admin` and the TypeScript
   client in `tsworkspace/packages/sandbox-ws-client`.
3. Describe the rollout order in the commit body.

## Read the golden files

`rsworkspace/crates/sandbox/trogon-sandbox-contract/tests/wire_contract.rs` serializes the public command,
log and terminal messages and compares them with the JSON files in
`rsworkspace/crates/sandbox/trogon-sandbox-contract/tests/wire_fixtures/`. A renamed field, a removed
variant, a changed tag or a new always-serialized field changes the JSON and fails the test. Each case also
reads its own JSON back into the type.

When the test fails on a change you did not intend, revert the change. When the change is intended and
backward compatible, regenerate the files:

```sh
UPDATE_WIRE_FIXTURES=1 cargo test -p trogon-sandbox-contract --test wire_contract
```

Review the diff of `tests/wire_fixtures/*.json` in the same commit. An additive field with a default leaves
the files untouched. A diff that is not purely additive needs the rollout described above.

## Cover a new type

Add a case with `f.case("name", value)` to the matching test in `tests/wire_contract.rs`, or a new test with
its own `Fixture::new("file.json")`, and run the regeneration command once. List a new fixture file in the
`data` entry of the `wire_contract` test in the crate's `BUILD.bazel` only when it is outside
`tests/wire_fixtures/`.

## Check the change

Run the tests of both crates, which cover serialization and the subject
layout, and the crates that consume the type:

```sh
cargo test -p trogon-sandbox-contract -p trogon-sandbox-protocol
cargo test -p trogon-sandbox-broker
```

Cover every variant of a new enum and a value with each optional field both set
and unset.
