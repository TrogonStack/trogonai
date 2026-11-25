# SPIRE deployment assets

The renderer prepares the Kubernetes profile described in
[the operator guide](../../../docs/how-to/deploy-workload-identity.md).
It requires exact release signature verification before emitting deployment
output. An image digest or release workflow source alone cannot pass that gate.

The example describes resource identities and references existing credentials;
it contains no production credentials. Replace its example values with reviewed
deployment inputs. Rendering does not deploy resources or establish production
acceptance.

```sh
mise exec cosign@3.1.4 -- python3 devops/kubernetes/spire/render.py \
  --config deployment.json --output rendered-identity
```

The input must be tracked by git with no uncommitted changes (asked in the input
file's own repository), so entries are versioned and reviewed as code;
`--allow-unversioned-input` is the explicit escape for fixtures, the example and
tests. `registration-plan.json` records the input's SHA-256 and commit (`input`)
and the SHA-256 of the rendered `registrations.json` bytes
(`registrations_sha256`), the digest a reviewer approves.

The renderer also emits `alert-rules.json` (Prometheus rules, also shipped as a
ConfigMap in the monitoring namespace) built by `alerts.py` from the documented
SPIRE telemetry metric names, Prometheus telemetry blocks in `server.conf` and
`agent.conf`, and, from `upstream_authority`, the SPIRE `aws_pca`
UpstreamAuthority with its scoped IAM statement. `assurance_level` `regulated`
requires the upstream authority; a `disk` upstream authority is refused.

`reconcile.py` compares a rendered `registrations.json` with the live
`spire-server entry show -output json` listing and applies the difference, so a
retired registration is deleted from the server; see the operator guide's
reconciliation section. The rendered server listens on
`/run/spire/admin/api.sock`, not the CLI default, so pass `--socket-path`; it is
added after the subcommand of every `spire-server` call:

```sh
python3 devops/kubernetes/spire/reconcile.py --registrations rendered-identity/registrations.json \
  --server-command 'kubectl exec -i -n identity-control spire-server-0 -c server -- /opt/spire/bin/spire-server' \
  --socket-path /run/spire/admin/api.sock --check
```

Apply, which is none of `--dry-run`, `--check` or `--current`, requires
`--approved-digest <sha256 hex>` and exits 1 before contacting the server unless
it equals the SHA-256 of the `--registrations` bytes.

`--dry-run` prints the plan and always exits 0. `--check` prints the plan, changes
nothing, and exits 2 when the plan is not empty or any live entry, owned or
foreign, has fewer than two selectors (listed as `broad_selectors`). It exits 3
(`privileged grant present`) when the server holds an entry with `admin` or
`downstream` set, naming each entry ID, SPIFFE ID and flag, and takes precedence
over drift; every other refused listing exits 1. Exit 3 also ends `--dry-run` and
apply, which never touch such an entry. Drop `--check` to apply.

The migration Job leaves a bundle for its disposable trust domain
(`migration.invalid`) in the datastore, and the real servers report it as a
stored federated bundle. `reconcile.py` also lists `spire-server bundle list
-output json`, reports every stored bundle as `undeclared_bundles` (the compiler
renders no federation), and the first apply deletes the migration one with
`bundle delete -mode restrict`. `--check` exits 2 while any bundle is stored. The
tool never deletes a foreign bundle it does not recognise: apply exits 1 naming
its trust domain. With `--current` bundles are not inspected and the plan says
so (`"bundles_inspected": false`).

Each registration lists `node_names`, a non-empty subset of the deployment
`node_names`. The renderer emits one node alias per node (the `k8s_psat`
selectors plus `agent_node_name:<node>`) and one entry per registration and
allowed node, parented to that node's alias, so the same SPIFFE ID appears under
several parents and a node outside the list receives no SVID. An upgrade from the
single cluster alias deletes the old alias and its entries and creates the new
ones in one run. `api_server_port` is the port the server NetworkPolicy opens to
`api_server_cidrs`; use the endpoint port the CNI sees after translation (443
for the service address, usually 6443 for the endpoint address). Workload paths
refuse StatefulSet ordinals, IP-shaped and date-shaped segments, long digit runs,
ReplicaSet hash suffixes and segments containing a node name; `v2` stays legal.

The server runs with `disable_migration` set, so the renderer also emits a
one-shot Job (`spire-server-migrate-<spec hash>`, same image, same datastore
TLS, covered by the NetworkPolicies and admission policy). Apply the Job, wait
for `condition=complete`, then apply the StatefulSet and DaemonSet. The Job
attests nothing and uses a throwaway in-memory key manager. A Job's pod template
is immutable, so the name hashes the whole Job spec and `migrate.conf`: any
change gets a new Job, and an unchanged render re-applies as a no-op. Delete a
completed Job from an earlier render once its replacement completes.

`credential_inventory.py` validates the operator-owned inventory of static
service credentials (`credential-inventory.example.json` shows the shape) against
the rendered registrations and prints the counts, including retired credentials
against the inventory. A path past `static` needs a registered caller SPIFFE ID,
and `retired` needs one full CA rotation (`CA_TTL_SECONDS` in `profile.py`) after
enforcement started. It exits 1 naming the path and the rule:

```sh
python3 devops/kubernetes/spire/credential_inventory.py \
  --inventory credential-inventory.json --registrations rendered-identity/registrations.json
```

`peer_id_evidence.py` reads the saved JSON log of the egress proxy from the dry
run pilot and refuses unless, for every call path in state `parallel` (or each
`--path`), at least one `peer_connection_accepted` event names the path's
`caller_spiffe_id` and no `egress_would_deny` event names it. A line it cannot
parse is refused with its number, never skipped. `--strict` also refuses a peer
ID that no inventory path expects. It prints the SHA-256 of the log it accepted;
record it as `peer_id_evidence_sha256` on the call path. The inventory refuses
`enforced` and `retired` without that digest, and with `--evidence-dir` it finds
the file by digest and checks it again. The log is the proxy's side only:

```sh
python3 devops/kubernetes/spire/peer_id_evidence.py \
  --inventory credential-inventory.json --registrations rendered-identity/registrations.json \
  --proxy-log proxy.jsonl --strict
```

For each entry it deletes, `reconcile.py` logs `deleted entry <id> at <UTC>
x509_svid_ttl_seconds=<n> last_svid_expires_at=<UTC> time_to_revoke_seconds=<n>`,
then `time to revoke maximum_seconds=<n> entries=<n>`. The time to revoke is the
entry's own X.509-SVID TTL as the server listed it, or the rendered server
default when the entry carries none: the last SVID issued for the entry can
expire no later than that long after the delete.

`upstream_ca_evidence.py` refuses unless the saved output of `aws acm-pca
describe-certificate-authority` shows the configured authority, `ACTIVE`, with
the key storage level the assurance level requires (`regulated`: FIPS 140-2 level
3 or higher, or the CCPC equivalent). The fixture in `testdata/` follows the AWS
documented response, not a live call:

```sh
python3 devops/kubernetes/spire/upstream_ca_evidence.py \
  --config deployment.json --describe describe-certificate-authority.json
```

`iam_evidence.py` refuses unless the saved outputs of `aws iam get-role`, `aws iam
get-role-policy`, `aws iam list-role-policies`, `aws iam list-attached-role-policies`
and `aws kms get-key-policy` (once per signing key) equal the rendered trust,
permission and key policies exactly: no extra principal, action, resource,
condition or statement, one inline policy, no managed policy. Permission boundaries
and service control policies are not visible in that output. The fixtures in
`testdata/` follow the AWS documented shapes, not a live call:

```sh
python3 devops/kubernetes/spire/iam_evidence.py --config deployment.json \
  --role iam-get-role.json --role-policy iam-get-role-policy.json \
  --role-policies iam-list-role-policies.json \
  --attached-policies iam-list-attached-role-policies.json --key-policy kms-get-key-policy.json
```

`server_operator_groups` (required, non-empty, never `system:authenticated`,
`system:unauthenticated` or `system:serviceaccounts`) names the groups allowed to
exec into, attach to, port-forward to or add ephemeral containers to pods in the
server and agent namespaces. A rendered admission policy refuses everyone else, so
the operator who runs `reconcile.py` through `kubectl exec` must be in one of them.

Two preconditions are not created by the compiler. The alert-rules ConfigMap
targets the `monitoring` namespace, which must exist. The cluster-wide admission
policy blocks dynamic provisioners that run helper pods (host paths, privileged
helpers), so use pre-provisioned or CSI volumes without such pods.

`test_reconcile.py` runs against server output captured
from SPIRE 1.16.0 in `testdata/`. `test_reconcile_live.py` applies, changes and
retires entries on a real server in a throwaway Docker container, and shows
that the server honours the rendered `tls_config`: it accepts a TLS 1.3
handshake and refuses TLS 1.2, which a control server without the setting
accepts. It is skipped unless `SPIRE_RECONCILE_LIVE=1`:

```sh
SPIRE_RECONCILE_LIVE=1 mise exec -- python3 -m unittest discover -v \
  -s devops/kubernetes/spire -p 'test_reconcile_live.py'
```

`test_jwt_bundle_live.py` runs the Rust JWT bundle source and JWT-SVID verifier
(`trogon-workload-identity/tests/live_spire_agent.rs`) against a real pinned
SPIRE server and agent in throwaway Docker containers. The test runs in a Rust
container that shares the agent's socket volume and PID namespace, and the JWT-SVID
comes from `spire-agent api fetch jwt`. It uses join token node attestation and
in-memory keys instead of the production attestor and KMS-backed keys. It is skipped
unless `SPIRE_JWT_LIVE=1`, removes its containers, network and socket volume, and
keeps the cargo build cache volumes `trogon-jwt-live-target` and
`trogon-jwt-live-cargo-registry`:

```sh
SPIRE_JWT_LIVE=1 mise exec -- python3 -m unittest discover -v \
  -s devops/kubernetes/spire -p 'test_jwt_bundle_live.py'
```

`test_cluster_live.py` renders the deployment onto a fresh kind cluster with a
TLS PostgreSQL. It runs the rendered `aws_kms` and `aws_pca` plugin blocks, and
every NetworkPolicy and admission policy, as rendered, against an in-cluster moto
emulator pinned by digest. The only server change is the environment variables
`AWS_ENDPOINT_URL` and `AWS_ENDPOINT_URL_STS`; the input points
`aws_endpoint_cidrs` and `certificate_authority_arn` at the emulator and
`server_operator_groups` at the kind admin's groups; cosign verification is
stubbed; and three TTLs are shortened (`agent_ttl`, `default_x509_svid_ttl` and
one entry TTL). It does not prove IAM authorisation, the KMS HSM boundary, real
ACM PCA behaviour, STS or OIDC validation, or TLS to AWS.
It proves that the manifests are admitted and the agents attest with `k8s_psat`,
that the rendered migration Job completes, that `reconcile.py` converges through
`kubectl exec` (including an upgrade from the single alias), that a pilot pod
receives exactly its own X.509-SVID on an allowed node and none on another,
that `promtool check rules` and `promtool test rules` (a new series at 1, a flat
series, an increment, a counter reset) accept the shipped rules, that every series
used by `alert-rules.json` exists after its event (a join token, an out-of-band
admin and downstream entry, a federation fetch attempt), that `--check` exits 3 on
the privileged entries, that an impersonated user outside `server_operator_groups`
is refused exec, attach, port-forward and ephemeral containers, that SVIDs rotate under load and survive a server outage until
expiry, and that the admission policies refuse violating pods. It is skipped
unless `SPIRE_CLUSTER_LIVE=1` and kind, kubectl and Docker are available, and it
deletes its cluster when it ends:

```sh
SPIRE_CLUSTER_LIVE=1 mise exec -- python3 -m unittest discover -v \
  -s devops/kubernetes/spire -p 'test_cluster_live.py'
```

Configuration and policy regressions run locally without an issuer:

```sh
mise exec -- python3 -m unittest discover -s devops/kubernetes/spire -p 'test_*.py'
mise exec go@1.27.3 python@3.14.5 -- python3 devops/kubernetes/spire/check_policies.py
```

The policy runner requires the reviewed fixture at
`goworkspace/internal/spirepolicy/testdata/deployment.json` to match fresh output
from the production builders exactly. CI runs the fixture comparison
(`test_check_policies.py` in `//devops/kubernetes/spire:deployment_test`) and the Go
suite (`//goworkspace/internal/spirepolicy:spirepolicy_test`) on every pull
request and every push to main, through `bazel coverage //...` in
`.github/workflows/ci-bazel.yml`. The command runs the same thing locally. It writes the generated
resources and supervisor patches into a private temporary JSON fixture, passes
its path to the Go suite through `SPIRE_POLICY_FIXTURE`, and removes it when the
suite exits. It runs
`go test -count=1` with local toolchain selection and read-only modules, so each
invocation evaluates fresh input and returns the test process's exit status.
Fixture generation does not assert release trust or emit verification evidence.

Ordinary `go test ./...` in `goworkspace` uses the reviewed fixture without
Python or an environment variable. When a reviewed generator change requires a
fixture update, regenerate it and inspect the diff before running the policy
checker again:

```sh
mise exec python@3.14.5 -- python3 devops/kubernetes/spire/check_policies.py --write-fixture
```

The CEL suite evaluates the generated admission expressions with the CEL version
used by Kubernetes 1.37. Production acceptance additionally requires API-server
type checking and actual admission attempts in the selected cluster.

The admission baseline applies cluster-wide and allows host privileges only
for the exact generated SPIRE agent profile. It also bounds issuer, agent and
supervisor node selection through pod and binding admission. Privileged system
components need separately reviewed exact profiles and live recovery evidence
before rollout; namespace-wide exemptions are not supported. Existing pods and
PVC/CSI backing storage require independent isolation validation. Follow the
operator guide's infrastructure inventory and enablement gates before applying.

An optional rebuild-input preparer authenticates official release archives and
a pinned distroless base, then emits Dockerfiles, binaries, license and input
verification receipts. It never builds or publishes an image and does not
change the deployment renderer's trust policy:

```sh
mise exec cosign@3.1.4 -- python3 devops/kubernetes/spire/prepare_images.py \
  --architecture arm64 --scratch-root scratch --output authenticated-build-inputs
```

The scratch directory must exist. GitHub's authenticated release-publisher
attestation establishes the archive's release origin and bytes. It does not
establish SLSA build provenance. A downstream image still needs approved build
and signature evidence for its exact digest before deployment.
