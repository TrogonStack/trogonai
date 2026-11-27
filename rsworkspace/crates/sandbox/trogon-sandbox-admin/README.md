# Sandbox admin

`trogon-sandbox-admin` is the operator command line for the sandbox stack. It talks to NATS
and JetStream directly, so it needs no running broker. The library holds each command as a
function that writes to a `Console`; the binary only parses flags and dispatches.

## Commands

- `api-key mint | list | revoke` manage the bearer keys stored in KV.
- `image-policy`, `host-mount`, `native-passthrough` and `tenant-limit` each have
  `set | show | clear` for one tenant.
- `quota show` prints a tenant's monthly usage.
- `lease clear-freeze | retry-freeze` resolve a pause or resume the broker gave up settling.
- `worker cordon | drain | uncordon | status | list` fence workers out of placement.
- `capability show | diff` read provider capability profiles from live worker heartbeats.
- `credential set | list | rotate | revoke-token | migrate` manage sealed credentials.
- `tag set | get | list | history | clear` manage snapshot tags and read their audit trail.
- `prewarm set | show | clear` manage warm templates.
- `snapshot delete | status` remove a snapshot artifact or read a capture result.
- `substrate prebuild` builds a substrate template.

## Connection

`--nats-url` (`NATS_URL`) defaults to the local server. `--api-key-bucket`
(`SANDBOX_API_KEY_BUCKET`) selects the key bucket. The credential commands seal and open
material with the key in `--kms-key` (`SANDBOX_KMS_KEY`), the base64 of 32 bytes.
`credential set` reads the material from standard input.

## Types

- Tenants, digests, lease ids, worker ids, provider tokens and tag names are parsed into
  their contract types while the arguments are read, so a malformed value fails before any
  connection is made.
- `AdminError` names each way a command can fail. `AdminError::Step` carries the failing
  operation and its source.
- `Console` separates results, written to standard output, from notices for the operator,
  written to standard error. A minted token goes to standard output only.
- `PrebuildRequest` carries the typed inputs of `substrate prebuild`.

## Tests

The integration tests start NATS in a container through Docker.
