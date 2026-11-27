# trogon-sandbox-aca-worker

A sandbox provider worker that runs each lease in an Azure Container Apps Dynamic Sessions session.

## Types

| Type | Role |
| --- | --- |
| `AcaWorkerProvider` | The `Provider` implementation: allocates a session per lease, prepares its workspace and deletes it on cancel. |
| `AcaSessionApi`, `ReqwestAcaClient` | The Dynamic Sessions data plane, as a trait and its HTTP implementation. |
| `CredentialIssuer`, `BrokerCredentialIssuer` | How the worker gets Azure credentials: a trait and its NATS credential broker implementation. |
| `VolumeBackends`, `AzureVolumeBackends` | Which object store a volume workspace syncs with. |
| `run_worker` | Subscribes to dispatch and cancel subjects and serves them until cancelled. |
| `serve` | Connects to NATS, starts heartbeats and calls `run_worker`. |
| `run_cleanup_retries` | Retries the cleanup of allocations that failed and could not delete their session. |
| `WorkerArgs`, `ProviderConfig` | Command line and environment configuration of the binary. |
| `AcaError` | Why an Azure Container Apps operation failed. |

## Behavior

- A lease allocates one session in the configured pool, waits for it to become ready, lays the workspace out in a
  scratch directory, uploads its files and tells the session which image the lease asked for.
- Workspaces can be empty, a shallow git clone, or an object store volume. A read-write volume is pushed back when
  the lease is cancelled. Host mounts are refused.
- The data plane bearer comes from a service principal the credential broker issues, exchanged at the login authority.
- Leases that ask for a snapshot image, environment variables, remote mounts or privileged mode are refused, because
  Dynamic Sessions cannot honour them.
- Cancelling a lease deletes its session. An allocation that fails deletes its session too. If that deletion fails
  the lease stays held and the cleanup is retried on a timer.
- A redispatched lease launches once until it is cancelled.
- Lifecycle events are published to JetStream, and heartbeats advertise capacity and, optionally, the Azure region.
- Volume credentials are issued by the broker and checked, but the volumes crate builds its Azure store without a
  credential parameter, so the store authenticates through its own environment.

## Fit

The worker implements the provider side of `trogon-sandbox-protocol` for the `azure_container_apps` provider kind.
It reads leases and writes lifecycle events in the types of `trogon-sandbox-contract`, asks the credential broker
through `trogon-sandbox-credential-client`, syncs volumes with `trogon-sandbox-volumes` and reports through
`trogon-sandbox-observability`.

## Running

```sh
SANDBOX_ACA_POOL_MANAGEMENT_ENDPOINT=https://<region>.dynamicsessions.io/subscriptions/<id>/resourceGroups/<group>/sessionPools/<pool> \
SANDBOX_ACA_SESSION_POOL_NAME=<pool> \
cargo run -p trogon-sandbox-aca-worker
```

| Variable | Default | Meaning |
| --- | --- | --- |
| `SANDBOX_NATS_URL` | `nats://127.0.0.1:4222` | NATS server to serve from. |
| `SANDBOX_ACA_POOL_MANAGEMENT_ENDPOINT` | required | Management endpoint of the session pool. |
| `SANDBOX_ACA_SESSION_POOL_NAME` | required | Name of the session pool. |
| `SANDBOX_ACA_LOGIN_ENDPOINT` | `https://login.microsoftonline.com/` | Authority service principal tokens are requested from. |
| `SANDBOX_ACA_DEFAULT_IMAGE` | unset | Image a lease boots when its own reference names none. |
| `SANDBOX_ACA_AZURE_CREDENTIAL_ID` | `azure-sp` | Broker secret holding the service principal. |
| `SANDBOX_ACA_VOLUME_CREDENTIAL_ID` | `azure-volume` | Broker secret holding the volume storage credential. |
| `SANDBOX_ACA_VOLUME_STORAGE_ACCOUNT` | unset | Storage account volumes live in. Without it volume workspaces are refused. |
| `SANDBOX_ACA_REGION` | unset | Azure region advertised in heartbeats. |
| `SANDBOX_MAX_LEASE_LIFETIME_SECONDS` | `86400` | Longest a lease may live. |
| `SANDBOX_MAX_CONCURRENT_LEASES` | `4` | Capacity advertised in heartbeats. |
| `SANDBOX_DISPATCH_FENCE_FILE` | unset | Journal that keeps the dispatch fence across restarts. |

## Tests

```sh
cargo test -p trogon-sandbox-aca-worker
```

Azure is faked in process, so no test reaches the internet. The NATS tests start an ephemeral JetStream server through
`trogon-nats` with the `test-support` feature, which needs Docker.
