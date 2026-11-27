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

## Check the change

Run the tests of both crates, which cover serialization and the subject
layout, and the crates that consume the type:

```sh
cargo test -p trogon-sandbox-contract -p trogon-sandbox-protocol
cargo test -p trogon-sandbox-broker
```

Cover every variant of a new enum and a value with each optional field both set
and unset.
