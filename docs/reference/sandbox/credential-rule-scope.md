# Credential rule scope and the resolution fence

A `SecretBinding` decides which hosts receive a credential through `inject_hosts`. Three optional fields narrow that to
the request, and a fence on the credential request narrows what the broker will resolve. Every one of these fields is
serde-defaulted, so existing bindings and clients keep their behavior until they opt in.

## Request scope

`inject_scope` is flattened into the binding.

| Field | Type | Default | Meaning |
|-------|------|---------|---------|
| `path_prefix` | string | absent | Only requests at or below this path receive the credential. |
| `methods` | non-empty list of HTTP methods | absent | Only requests with one of these methods receive the credential. |
| `scope_version` | `1` or `2` | `1` | `1`: a host pattern without a port spans every port. `2`: it means port 443. |

The proxy enforces these on terminated requests, and on plain-HTTP proxy requests when plaintext injection is on, after
the existing Host and target validation. It decides per request, so a keep-alive connection can carry a matching and a
non-matching request. The path is the request target without its query.

### Path prefix

Matching is segment aware. `/v1` admits `/v1` and `/v1/models` and refuses `/v10`. A prefix:

- must start with `/` and be at most 1024 bytes;
- may use only ASCII letters, digits, unreserved characters, sub-delimiters, `:`, `@` and `/` (no `%`, `?`, `#`, spaces
  or control characters);
- must not end with `/` unless it is `/`;
- must not contain empty, `.` or `..` segments.

### Methods

The set is closed: `GET`, `HEAD`, `POST`, `PUT`, `PATCH`, `DELETE` and `OPTIONS`, in upper case. An unknown method name
is rejected when the binding is admitted. A request whose method is outside the set receives no credential.

### Port default

Under `scope_version` 1 an inject host without a port matches any port, as it always did. Under `2` it matches only 443.
Set `2` on a binding to opt in; nothing changes for bindings that do not carry the field. Ship the proxy first, then
move bindings to `2` one at a time.

### Connection-level decisions

Where the proxy has no request yet (CONNECT, TLS handshake, strip rules), a binding with a path or method scope is
treated as "may apply" for planning but never injects without a per-request match. Response header strip rules stay at
host level.

## Resolution fence

A credential request may carry a `PolicyFence { lease_id, policy_revision }` (`CredentialIssueRequest.fence`, absent for
older clients). The broker looks up what the control plane committed for that tenant and lease, and resolves the
reference only when the committed revision equals the one in the fence and the committed policy names the reference. A
stale revision, a revision ahead of the commit, an unknown or revoked lease, another tenant's lease and an unnamed
reference all return the same `PolicyFenceRefused` error.

| Setting | Values | Default | Effect |
|---------|--------|---------|--------|
| `--fence-mode` / `SANDBOX_CREDENTIAL_FENCE_MODE` (credential broker) | `optional`, `required` | `optional` | `optional` accepts a request with no fence (older clients) and verifies one that has it. `required` refuses an unfenced request. |
| `EGRESS_PROXY_CREDENTIAL_BROKER_LEASE_ID` and `EGRESS_PROXY_CREDENTIAL_BROKER_POLICY_REVISION` (egress proxy) | lease id, integer | unset | When both are set the proxy stamps every credential request with the fence. |

### Who writes the committed policy

The broker control plane (`trogon-sandbox-broker`) writes it. For each lease whose spec names credential references
(`Transform` rules and secret bindings), it commits the set of `pull_secret_id` values before the lease is dispatched,
so a worker never starts against a missing record. The first commit is revision 1. Each network policy update that
changes the set commits the next revision before the update is applied to the lease. Committing the same set again keeps
the revision, so a retried create or update does not move it. Releasing or failing the lease deletes the record.

The record lives in the NATS JetStream KV bucket `sandbox-committed-credential-policies` (the
`COMMITTED_POLICY_BUCKET` constant in `rsworkspace/crates/sandbox/trogon-sandbox-protocol/src/constants.rs`). The key is
the hex of the tenant and the hex of the lease, joined by a dot. The credential broker reads it through a
`CommittedPolicyStore`. The control plane writes it through a `CommittedPolicyWriter`, using compare-and-swap so a stale
writer cannot roll the revision back.

Workers read the same record when they build a lease's egress proxy and stamp the committed revision on every credential
request. This covers the subprocess worker (lease secrets and the lease proxy) and the Firecracker worker. A worker
fences a lease only when the lease has credential references.

| Setting | Component | Default | Effect |
|---------|-----------|---------|--------|
| `--commit-credential-policies` / `SANDBOX_COMMIT_CREDENTIAL_POLICIES` | `trogon-sandbox-broker` | `true` | Commit and revoke policies. Takes effect only with the `nats-kv` quota backend; with the in-memory backend there is no shared bucket and nothing is written. |
| `--require-committed-policy` / `SANDBOX_REQUIRE_COMMITTED_POLICY` | `trogon-sandbox-subprocess-worker`, `trogon-sandbox-firecracker-worker` | off | Refuse a lease that has credential references but no committed policy. Off, the lease runs with unfenced requests. |
| `--fence-mode` / `SANDBOX_CREDENTIAL_FENCE_MODE` | `trogon-sandbox-credential-broker` | `optional` | `required` refuses any request without a fence. |

The binaries default to the permissive values, except the broker commit flag, so an upgrade does not break a running
deployment. The Helm charts under `devops/helm/charts` set the strict values (`commitCredentialPolicies: true`,
`requireCommittedPolicy: true`, `fenceMode: required`).

### Rollout order

1. Upgrade `trogon-sandbox-broker` with `SANDBOX_COMMIT_CREDENTIAL_POLICIES=true` and the `nats-kv` quota backend. New
   leases now have a committed policy.
2. Upgrade the workers. Leases created before step 1 have no record, so keep `SANDBOX_REQUIRE_COMMITTED_POLICY` off until
   those leases are gone, then turn it on.
3. Confirm the credential broker sees no unfenced traffic, then set `SANDBOX_CREDENTIAL_FENCE_MODE=required`.

Turning `required` on before step 1 or 2 makes credential requests fail.

### Known limitation: stale proxy after a failed update

The broker commits the new revision before it applies a policy update, and the credential broker accepts only a request
whose revision equals the committed one. If the commit succeeds and a later step fails (for example the worker refuses
the update), the running proxy keeps the old revision and its credential requests are refused until the next successful
update. The failure is closed, not open.

## See also

- [Add a pull secret](../../how-to/sandbox/add-a-pull-secret.md)
- `rsworkspace/crates/sandbox/trogon-sandbox-contract/src/inject_scope.rs`,
  `rsworkspace/crates/sandbox/trogon-sandbox-credential-broker/src/committed_policy.rs`,
  `rsworkspace/crates/sandbox/trogon-sandbox-broker/src/core/credential_policy.rs`
