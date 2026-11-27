# Sign lease dispatches and verify them on workers

The broker signs each lease dispatch with an Ed25519 key, and every worker verifies the signature before it acts. The signature binds the tenant, lease id, operation, target worker and attempt, an expiry, and a digest of the whole dispatch body. A worker also refuses a tenant that differs from the one it first saw for the same lease id.

The signed envelope is the `dispatch_auth` object added to the dispatch JSON. Workers that predate it ignore it, so rollout order does not affect compatibility. Cleanup, per-lease requests (run command, file operations, pause and the rest), SSH attach and detach, snapshot export and native snapshot deletion carry the same kind of envelope in a `Sandbox-Dispatch-Auth` NATS header, so their bodies reach the worker unchanged.

## Configuration

Broker:

| Variable | Meaning |
| --- | --- |
| `SANDBOX_DISPATCH_SIGNING_KEY_FILE` | Path to a file holding the base64 32-byte seed. Preferred. |
| `SANDBOX_DISPATCH_SIGNING_KEY` | The same seed in the variable itself, for secret stores that inject environment variables. |
| `SANDBOX_DISPATCH_SIGNING_KEY_ID` | Key id carried in each envelope. Default `k1`. |
| `SANDBOX_DISPATCH_TTL_SECS` | Envelope lifetime. Default 300. A re-sent dispatch is signed again. |

With no key set the broker sends unsigned dispatches.

Workers:

| Variable | Meaning |
| --- | --- |
| `SANDBOX_DISPATCH_PUBLIC_KEYS_FILE` | File with one `key_id:base64-public-key` per line (`#` comments allowed). |
| `SANDBOX_DISPATCH_PUBLIC_KEYS` | The same entries comma separated. |
| `SANDBOX_DISPATCH_VERIFY` | `off`, `warn` or `require`. Without it the binary runs in `warn`. |
| `SANDBOX_DISPATCH_FENCE_FILE` | Where the lease-to-tenant and attempt fence is kept across restarts (also `--dispatch-fence-file` on the workers that take it). The journal file is named `dispatch-fence.jsonl`. With no path configured the fence is memory only. |

The SSH gateway and the snapshot builder also sign, because they send SSH attach and detach and snapshot export. They read the same `SANDBOX_DISPATCH_SIGNING_KEY`, `SANDBOX_DISPATCH_SIGNING_KEY_FILE`, `SANDBOX_DISPATCH_SIGNING_KEY_ID` and `SANDBOX_DISPATCH_TTL_SECS` as the broker, so they need the same key, or any key whose public half the workers list. Without a key they send unsigned requests.

In `warn` a missing or invalid signature is logged and the dispatch is accepted. In `require` it is refused with a dispatch rejection. A worker whose verification configuration cannot be read while `require` is set refuses every dispatch.

## Generate a key

```sh
openssl genpkey -algorithm ed25519 -outform DER -out dispatch.der
tail -c 32 dispatch.der | base64 > dispatch-signing.key      # broker seed, keep secret
openssl pkey -inform DER -in dispatch.der -pubout -outform DER | tail -c 32 | base64   # public key
rm dispatch.der
```

Store `dispatch-signing.key` in the broker's secret store and mount it at the path named by `SANDBOX_DISPATCH_SIGNING_KEY_FILE`. Publish the public key to workers as `k1:<base64>`.

## Roll it out

1. Generate the key and distribute the public key to all workers with `SANDBOX_DISPATCH_PUBLIC_KEYS_FILE`. Leave `SANDBOX_DISPATCH_VERIFY` unset so workers stay in `warn`.
2. Deploy the new worker build everywhere. Dispatches are still unsigned, so workers log a notice and accept.
3. Deploy brokers with the signing key. Every dispatch is now signed.
4. Watch worker logs for `accepting dispatch that failed verification`. Zero such lines over a release means every sender signs and every key is known.
5. Give the SSH gateway and the snapshot builder the signing key before moving workers to `require`. Otherwise SSH attach and snapshot export are refused. Then set `SANDBOX_DISPATCH_VERIFY=require` on workers, one provider at a time.

To rotate, add the new public key under a new id on workers, switch the broker's `SANDBOX_DISPATCH_SIGNING_KEY_ID` and key, then remove the old entry.

## What a worker checks

Every request is checked for a known key, a valid signature, an unexpired envelope and the exact subject it arrived on. Then:

- A lease dispatch also binds the body digest, the target worker and the attempt.
- A per-lease request or cleanup binds the lease id in its subject (or in the cleanup body), the body digest and a one-time nonce, so a captured request cannot be sent again. The envelope names the tenant that owns the lease.
- The worker remembers which tenant and attempt it first saw for each lease id. A signed request for another tenant, for an attempt other than the live one, or for an attempt already cleaned up, is refused. A replayed older dispatch is refused the same way.
- SSH attach and detach are signed by the gateway with the tenant of the authenticated API key, and the body must name the same lease as the subject. A tenant that does not own the lease is refused. The gateway does not know the attempt, so the attempt is not checked for these.
- Snapshot export is signed by the snapshot builder with the tenant of the released lease. The body's `lease_id` must match the signed lease. The builder signs every retry afresh, so a retry is not a replay.
- Native snapshot deletion is signed on `sandbox.provider.<provider>.native_snapshot_delete`. It carries the tenant and the opaque native reference in its body and has no lease identity. Client credentials may publish the broker's `sandbox.native_snapshot.delete` subject, never the provider subject.
- A chunked body (file transfers) is bound to the signature too. The signature covers a digest of the full body, and the receiver compares it with the reassembled bytes, so an altered chunk is refused in `require` and logged and accepted in `warn`.
- A request for a lease this worker never saw is accepted only with a valid signature that names a tenant. An unsigned one is refused in `require`, and a signature with an empty tenant is refused.

The fence is kept in a small journal, `dispatch-fence.jsonl`, so it survives a restart. Workers that take `--dispatch-fence-file` or `SANDBOX_DISPATCH_FENCE_FILE` keep the fence in memory when it is unset, so point it at a persistent volume to survive restarts.

In `require` a refused request with a reply subject gets an error reply (`dispatch_auth`) instead of a timeout. In `warn` it is logged and accepted.

## Shipped deployments run `require`

| Where | How |
| --- | --- |
| Helm chart (`devops/helm/sandbox`) | `global.dispatchAuth.verify: require`. The broker reads `SANDBOX_DISPATCH_SIGNING_KEY` and every worker reads `SANDBOX_DISPATCH_PUBLIC_KEYS` from the Secret named by `global.dispatchAuth.secretName` (default `sandbox-dispatch-auth`, keys `signing-key` and `public-keys`). |
| `trogon-sandbox-dev` | Generates a throwaway key on each run, gives the broker the seed and the worker the public key, and sets `require`. |
| Compose stack (`devops/docker/compose/sandbox/compose.yml`) and the smoke scripts | `devops/docker/compose/sandbox/dispatch-keys.sh` generates a key and exports the broker and worker variables. |

Create the Helm Secret before installing, because pods that reference a missing Secret do not start:

```sh
kubectl create secret generic sandbox-dispatch-auth \
  --from-file=signing-key=dispatch-signing.key \
  --from-literal=public-keys="k1:<base64-public-key>"
```

To roll out gradually, install with `--set global.dispatchAuth.verify=warn`, watch the worker logs as in the rollout steps, then upgrade without the override. The binaries default to `warn`, so a worker run by hand without these variables keeps working.

## Run the conformance suite against a `require` worker

The provider conformance suite in `rsworkspace/crates/sandbox/trogon-sandbox-conformance` talks to a worker directly, so against a `require` worker it fails unless it signs. Give it the same key variables the broker reads (`SANDBOX_DISPATCH_SIGNING_KEY_FILE` or `SANDBOX_DISPATCH_SIGNING_KEY`, and `SANDBOX_DISPATCH_SIGNING_KEY_ID` if the worker's key is not `k1`). The external-worker tests pick them up through `Target::signed_from_env`. Without a key the suite stays unsigned. The worker's `SANDBOX_DISPATCH_PUBLIC_KEYS` must list the matching public key.

## Scope

Signed and checked on every worker: lease launch dispatches (including a targeted re-send), cleanup, every per-lease broker verb, SSH attach and detach, snapshot export, and the content of a chunked file transfer.

The broker signs a request for a lease it no longer holds with the tenant it remembers from earlier requests (bounded, kept in memory), without a worker or attempt. A lease the broker never saw, or one forgotten after a broker restart, goes out unsigned, which `require` workers refuse.

Not signed:

- Other worker-wide requests that name no lease.
- Replies. Workers do not sign what they send back.
- Frames on an SSH session stream after the attach. They are bound to the session the attach created.
