# Sandbox Kubernetes worker

`trogon-sandbox-k8s-worker` is a sandbox worker for Kubernetes. It leases one Job and
pod per sandbox, answers lease operations over NATS, and recovers what it created
after a restart. The `trogon-sandbox-k8s-worker` binary runs it.

## Types

- `K8sWorkerProvider` implements `trogon_sandbox_protocol::Provider` on top of the
  Kubernetes micro-VM provider. It launches and releases leases, runs and
  supervises commands, writes and reads files, manages accounts and network
  policies, takes checkpoints and rolls back to them, and claims warm pods.
- `WarmPool` keeps idle pods ready to claim. `WarmPoolConfig`, `WarmSlotState` and
  `WarmPodClaim` describe its sizing and what each slot holds.
- `DigestCache` remembers resolved image digests.
- `K8sVerb` names the lease operations the worker serves, and `run_worker` serves
  them: lease dispatch, cancellation, snapshot export and every verb in
  `K8sVerb::ALL`. An operation for a lease the worker does not hold gets no answer,
  so the worker that holds it answers.
- `K8sTerminals` serves interactive terminals, `K8sSshHost` the SSH bridge, and
  `K8sAuthorityHost` the authority check.
- `WorkerArgs` is the command line and environment configuration, and
  `WorkerIdentity` the parsed identity that labels everything the worker owns.
- `serve` starts a worker and returns a `ServeError` when it cannot. `profile` and
  `profile_for` report the capabilities the worker advertises, and
  `build_heartbeat` builds its heartbeat.
- `LeaseRecovery` counts what recovery found, and `K8sWorkerError`, `WorkerError`,
  `WarmPoolError`, `VolumeSyncError`, `SnapshotTransferError` and
  `WorkerIdentityError` are the typed errors.

## Behavior

- A lease runs in its own pod, labelled with the worker identity, so a worker only
  ever releases what it created. On start the worker recovers the leases it owns
  and sweeps the ones that expired.
- Command output is capped per request. A reply that would exceed what one NATS
  message can carry is trimmed, keeping the head and tail, and marked truncated.
- Rollback state can be persisted so a restarted worker still knows the
  checkpoints of its leases.
- Image signatures are checked against a cosign public key when one is configured.

## Fit

The wire types come from `trogon-sandbox-contract`. Dispatch, the authority guard,
heartbeats and large-payload transfer come from `trogon-sandbox-protocol`.

## Running

The binary reads its settings from flags or `SANDBOX_*` environment variables. The
namespace (`SANDBOX_K8S_NAMESPACE`) and the image (`SANDBOX_IMAGE`) are required, and
the cluster comes from the usual kubeconfig. It stops on interrupt.
