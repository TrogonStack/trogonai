# Sandbox E2B worker

`trogon-sandbox-e2b-worker` is a NATS-subscribed worker that fulfills sandbox lease
requests through the E2B API. It ships as the `trogon-sandbox-e2b-worker` binary and
as a library whose `launch` function runs the same worker in process.

The worker advertises the E2B provider capability profile, publishes heartbeats, and
serves lease, exec, file, terminal, watch, snapshot, and native-handle requests for the
leases it holds. Each lease maps to one E2B sandbox created from a template named by the
request's registry image. When the image names no repository, the worker falls back to
its default template; if that is empty too, the lease is refused with
`E2bProviderError::TemplateRequired`.

## Running

```text
trogon-sandbox-e2b-worker [OPTIONS]
```

`WorkerArgs` reads each option from the command line or its environment variable. The
defaults are a template of `base`, 4 concurrent leases, a 300 second HTTP timeout, and
the local NATS URL. The E2B API key comes from the environment, never from an argument.
`AuthorityMode` is read through `ReadEnv`, so tests can inject it.

When no dispatch ledger file is given, the worker id is a freshly generated ULID.
When the provider keeps a dispatch ledger, heartbeats announce the
`WorkerDispatchProtocol::TargetedV1` protocol; otherwise they announce `Legacy`.

## Custody

Two JetStream key-value buckets hold the state that must survive a restart. Both are
opened only when their backing stream is durable, and both refuse a bucket that is not.

- `assignments` records which lease a worker holds and which sandbox backs it, so a
  restarted worker can adopt or release it. Updates use compare-and-swap with bounded
  retries.
- `lease_cleanup` records a receipt for each lease being torn down, so a cleanup that
  was interrupted is finished once and confirmed absent exactly once.

`AssignmentsError`, `LeaseCleanupError`, and `CustodyBucketError` report failures from
these stores. `NativeSandboxIdError` rejects an E2B sandbox id the worker cannot record.

## Errors

`E2bWorkerError` wraps the typed failures of the worker loop: `Provider` carries an
`E2bProviderError` and `Dispatch` carries an `E2bDispatchError`. `LaunchError` covers
start-up failures: authority, configuration, API, worker id, ledger, dispatch,
connection, and worker.

## Testing

Unit tests live beside each module in `tests.rs`. HTTP calls to E2B are mocked with
wiremock and NATS behavior runs against a real JetStream test server.
