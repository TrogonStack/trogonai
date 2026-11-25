# Prepare a workload identity deployment

Use this procedure to prepare the Kubernetes PSAT and AWS KMS profile under
draft [ADR#0098](../adr/0098-attested-workload-identity.md).
The [workload identity acceptance contract](../reference/workload-identity.md)
remains the enablement gate. Generated manifests and local configuration tests
do not establish production attestation, isolation, availability or recovery.

The deployment renderer lives in `devops/kubernetes/spire/`. It pins SPIRE 1.16.0
and the exact server and agent image digests. Before producing any output, it
requires Cosign to verify those digests against the GitHub Actions OIDC issuer,
the exact SPIRE release workflow and tag, and its immutable source commit.
It also compares the verified payload digest with the image reference used in
the manifests. Verification failure produces no deployment output; there is no
skip-verification option.

Do not deploy this profile while verification of its pinned release artifacts
fails. Do not substitute a nightly signing identity, disable transparency-log
checks or infer provenance from a checksum. Updating a pin requires a reviewed
release change and fresh signature, configuration and security evidence.

If the upstream image cannot pass that gate, `prepare_images.py` provides a
separate source-preparation path. It verifies GitHub's signed release attestation
and the downloaded archive, pins the repository identity, tag object, source
commit and archive hash, and verifies a distroless base under its exact publisher
identity and digest. It emits architecture-specific Dockerfiles, authenticated
binaries and license material without building or publishing an image.
GitHub's release-publisher authentication is distinct from SLSA build provenance.
The resulting downstream image still needs its own approved builder provenance
and signature for the exact final digest. This preparation does not make the
original image trusted or permit a deployment-renderer fallback.

## Provision the independent deployment boundaries

Use a dedicated cluster and trust domain for each environment. Production and
nonproduction have distinct roots, registrations, databases, IAM roles and
platform recovery fences. The global admission policy deliberately protects
the selected identity plane across namespaces; this profile does not place
another environment's issuer on the same cluster.

Prepare the following outside the workload identity plane:

- An HA external PostgreSQL database with certificate authentication, encrypted
  storage and backups. Authorize the exact database role and client certificate.
  Require server hostname verification and retain the approved database CA.
  Coordinate schema migration through a fenced maintenance procedure before
  starting the HA servers. The server renders with `disable_migration: true`;
  the rendered one-shot migration Job is the step that migrates the schema (see
  [Apply order](#apply-order)).
- An EKS OIDC provider and a dedicated server IAM role. Its trust policy permits
  only the exact server service account and `sts.amazonaws.com` audience.
  Provision a distinct key administrator. Review the generated role and KMS
  key policies through the deployment's IAM validation and approval process.
- Independently authenticated bootstrap trust in the agent namespace, as an
  immutable ConfigMap containing `bundle.pem`. Record its public fingerprint
  through the approved bootstrap change process. The agent never downloads a
  first root from its unauthenticated server connection.
- The verified kubelet CA, as an immutable ConfigMap containing `ca.crt`. The
  kubelet must authenticate projected tokens and present a certificate valid for
  its configured node name. Insecure kubelet ports and skipped verification are
  excluded from this profile.
- Private endpoints or explicit endpoint addresses for the API server,
  PostgreSQL, KMS, regional STS and Resource Groups Tagging API. Network policies
  allow only the configured endpoint CIDRs and ports, plus cluster DNS. The
  server policy allows `api_server_cidrs` only on `api_server_port`. Set that
  port to the one the CNI sees after service translation: 443 for the service
  address and usually 6443 for the endpoint address. Prove the selected CNI
  enforces these policies, including service translation.
- Dedicated server nodes and separate workload-agent nodes across availability
  zones. Apply the configured NodeRestriction-protected pool labels through
  node administration, not a workload or kubelet identity. The configured node
  name allowlists must match those nodes.
- Retained issuer audit collection and monitoring. The server enables native
  JSON audit logs and renders Prometheus telemetry and alert rules. Log
  collection, retention, access control and a running Prometheus that loads
  the rules still need actual deployment evidence; logs alone do not implement platform activation
  acknowledgement. [Alert on issuer events](#alert-on-issuer-events) lists what
  to route.
- A `monitoring` namespace that already exists. The renderer ships the alert
  rules ConfigMap into it and does not create it, so apply fails until it exists.
- Volumes that do not need a dynamic provisioner running helper pods. The
  cluster-wide admission policy blocks provisioners that run helper pods with
  host paths or privileged helpers, so use pre-provisioned or CSI volumes.

The renderer references an existing database client Secret containing `tls.crt`
and `tls.key`, and a database CA ConfigMap containing `ca.crt`. It never accepts
or renders a password, token, private key or static AWS credential. Projected
service-account tokens have a finite lifetime. The AWS SDK uses the explicitly
configured web identity role, with instance metadata credential discovery
disabled.

The supported KMS plugin creates and rotates its signing keys. It needs key
management permissions, including `PutKeyPolicy` for keys it creates; it is not
a sign-only adapter to a preselected key. The generated policy limits key
creation to the selected trust-domain and environment tags and P-256 signing
keys, scopes managed keys and aliases, and keeps an independent recovery
principal. `ListAliases` and tagging discovery require account-level list
permissions. Validate actual IAM behavior, key ownership, cleanup and denial
before enablement. Do not describe this configuration as FIPS validated without
evidence for the actual deployed module and mode.

## Define governed registrations

Copy `deployment.example.json` to a private deployment input and replace every
example resource identity. Use exact trust domains, role ARNs, cluster OIDC
identity, dedicated node names, endpoint CIDRs, credential references and
trusted supervisor image digests. Set the recovery epoch from independently
governed recovery state. Set `assurance_level` and `upstream_authority` (see
the CA section), and name the `monitoring` namespace and scraper app label that
may reach the metrics ports. Set `api_server_port` to the API server port the
network policy opens, and give each registration its `node_names`. Every field
is required; a deployment input from before these fields existed is refused
until they are added.

Each transport SPIFFE ID follows the
[SPIFFE ID convention](../reference/workload-identity.md#spiffe-id-convention).
The renderer validates the raw string strictly and refuses a path that breaks
it. It refuses a query, fragment, userinfo, port, percent encoding, control
character, trailing slash, and empty, `.` or `..` segment, and a trust domain
other than the deployment's. The environment stays in the trust domain, and the
path has two to four segments of lowercase letters, digits, `_` and `-`, each
beginning and ending with a letter or digit and at most 63 characters. It also
refuses ephemeral values in a workload path: a StatefulSet ordinal suffix, an
IP-shaped or date-shaped segment, a run of six or more digits such as a
timestamp, a ReplicaSet hash suffix, a generated pod-name suffix, a UUID, a run
of 32 or more hex digits, and any segment containing an agent or server node
name. A version segment such as `v2` stays legal.

Each registration explicitly maps its transport SPIFFE ID to tenant, project,
role and, for an agent runtime, its canonical Ed25519 `did:key` application
identity. Service roles use a null application identity. An agent registration
targets its trusted host adapter; the guest never mounts the endpoint.
The mapping is governed state; the renderer does
not infer authority from a requester-selected URI path. Use the actual admitted
application identity rather than the example alias.

There is no cluster-wide node alias. The renderer emits one node alias per node
in `node_names`, each constrained by the PSAT cluster, exact agent namespace and
service account, the protected node-pool label and `agent_node_name:<node>`.
Each registration lists `node_names`, a non-empty subset of the deployment's
`node_names` without duplicates, and its workload entry is rendered once per
allowed node with that node's alias as parent. The same SPIFFE ID therefore
appears under several parents. A compromised node can obtain only the identities
whose registration lists it, and a node outside the list receives no SVID for
that workload. Workload entries require
the exact workload namespace, service account, container name, container image
and kernel UID together, which is five selectors that a test pins for every
workload entry. They have a one-hour X.509 ceiling, no administrative
privilege, no downstream signing and no federation entry. The image selector is
`k8s:container-image:<reference>@sha256:<digest>`; a tag-only or missing
reference is refused, because the SPIRE k8s attestor documents tag selectors as
non-deterministic. Confirm the selector matches the immutable image identity
actually reported by the kubelet; an index digest and a platform manifest digest
are not interchangeable labels. The Sigstore-backed `k8s:image-signature:verified`
selector is not rendered: it needs a `sigstore` block in the agent attestor plus
agent network access to the registry, Rekor and the TUF root, which this profile
does not allow.

`registrations.json` uses SPIRE's `entry create -data` format. Publish reviewed
entries through the protected administrative socket and a privileged operator
workflow. Neither agent nor supervisor service accounts receive registration
administration permissions.

### Reconcile and retire entries

Creating a file never removes anything, so a registration dropped from the
deployment input would stay valid on the server. Reconcile before every
publication. `reconcile.py` lists the live entries through the server command
you give it, plans the difference with the rendered `registrations.json`,
applies it, and lists again to confirm the server matches. Run it from a
privileged operator workflow, with the command that reaches the protected
administrative socket. The command must keep standard input open (`kubectl
exec -i`), because each entry is sent to `entry create -data -`:

```sh
python3 devops/kubernetes/spire/reconcile.py \
  --registrations rendered-identity/registrations.json \
  --server-command 'kubectl exec -i -n <control namespace> <server pod> -c server -- /opt/spire/bin/spire-server' \
  --socket-path /run/spire/admin/api.sock
```

The rendered server listens on `/run/spire/admin/api.sock`, not the CLI default,
so `--socket-path` is required; the tool adds it after the subcommand of every
`spire-server` call and requires an absolute path.

Apply is gated on review. The reviewer approves the `registrations_sha256` in
`registration-plan.json`, and apply needs that digest:

```sh
python3 devops/kubernetes/spire/reconcile.py ... \
  --approved-digest <registrations_sha256 from registration-plan.json>
```

Apply exits 1 before it contacts the server when the digest is missing or is not
the SHA-256 of the `--registrations` file bytes, so an edited or substituted file
is never applied. `--dry-run`, `--check` and `--current` change nothing and need
no digest.

Add `--dry-run` first to print the plan, including the ordered steps, without
changing the server; it always exits 0. `--server-command` defaults to `spire-server`, for a host
that has the socket. To plan offline from a saved listing, use `--current
current-entries.json` instead of `--server-command`; that mode never contacts a
server.

The apply order is: create new entries, node aliases first; then for each
changed entry delete the old one and create its replacement back to back; then
delete retired entries, workloads before their node alias. An entry whose
selectors, parent, TTLs, privilege flags or any other attribute differ is
replaced, not updated in place, so each live entry matches exactly one reviewed
definition. SPIRE rejects an entry identical in SPIFFE ID, parent and selectors,
so the old one must go first; expect a window of one call without that SVID for
each changed workload. Each step runs one command and the run stops at the first
failure with exit status 1 and a message naming the step. Every run plans from a
fresh listing, so rerunning after a failure resumes where it stopped; running it
twice with no change does nothing.

The tool owns the node aliases directly under the server inside
`<trust domain>/platform/agent-group/<cluster>/`, including aliases of earlier
recovery epochs and the single cluster alias of earlier releases, and every entry
parented inside that prefix. It never touches anything else. Upgrading from the
single cluster alias deletes that alias and its workload entries and creates the
per-node aliases and entries in the same run. It never creates an entry with
`admin` or `downstream` set, and refuses to run when the rendered set contains
one, contains no node alias, has node aliases that do not all belong to one
cluster, or when the listing is paginated, belongs to another trust domain, or
contains any admin or downstream entry, because the renderer never produces one;
investigate such an entry as a security event.

Run `--check` on a schedule or in the pipeline. It prints the plan, changes
nothing and exits 2 when the plan is not empty, so drift from the reviewed input
fails the run. It exits 3, naming each entry ID, SPIFFE ID and flag, when the
server holds an admin or downstream entry (`privileged grant present`), ahead of
any drift; every other refused listing exits 1. The exit codes are 0 converged, 1
refused or failed, 2 drift and 3 privileged grant, and `--dry-run` and apply also
exit 3 on such an entry. It also exits 2 when any live entry, owned or not, has fewer than two selectors; the plan lists
those as `broad_selectors`, and that count is the number behind "entries with a
single broad selector, target zero" in [Measure progress](#measure-progress).

The same listing includes the stored federated bundles
(`spire-server bundle list -output json`). The compiler renders no federation, so
every stored foreign bundle is undeclared: the plan lists them as
`undeclared_bundles`, and `--check` exits 2 while any remains. The migration Job
leaves a bundle for its disposable trust domain (`migration.invalid`) in the
datastore, so the first apply deletes it with `bundle delete -mode restrict`,
which the server refuses while any entry federates with it. The tool never
deletes a foreign bundle it does not recognise: apply finishes the entry changes,
then exits 1 naming those trust domains, and an operator reviews and removes
them. With `--current` the bundles are not inspected, and the plan says so with
`"bundles_inspected": false`.

Entries are compiled from reviewed, versioned input and applied by
`reconcile.py`. They are not derived by an in-cluster controller from workload
labels. The registration is governed state: the renderer does not infer authority
from a requester-selected path, and workload users cannot administer the issuer
or the dedicated supervisor namespaces, so a label a workload user can set never
becomes an identity.

Retirement trigger: delete an entry in the same change that retires its
workload, by removing the registration from the deployment input and running
the reconciliation. Do not leave an entry for a retired workload waiting for a
later cleanup.

The behavior is exercised against a real SPIRE 1.16.0 server in a throwaway
container by `test_reconcile_live.py`; see the SPIRE directory README. That
covers the `spire-server` command line, not your cluster's `kubectl exec` path,
so run a `--dry-run` through the production command before the first apply.

`registration-plan.json` preserves the intended application mapping and
recovery epoch as a human-reviewed plan. It is neither persisted live authority
nor a raw payload for the runtime registry.
The independent publisher must translate reviewed registration state to the
runtime's protobuf `RegistrationSnapshot` contract, preserve monotonic generations
and a fresh producer-issued lease no greater than 330 seconds, and
record administrative changes. Do not let the consumer publish or renew its
own admission authority.

Give each egress workload reader only the exact request APIs and its derived
`_INBOX.egress.<registration_bucket>.>` reply subscription. Use distinct reply
prefixes for provisioning, publication and emergency withdrawal. Audit every
untrusted role in the account for broad `_INBOX.>` and `>` subscriptions, or
place the authority services in separate accounts. Restricting stream APIs
without restricting reply subscriptions does not isolate their responses.

## Render and validate before applying

Run from the repository root with a trusted Cosign installation:

```sh
mise exec cosign@3.1.4 -- python3 devops/kubernetes/spire/render.py \
  --config deployment.json --output rendered-identity
```

The renderer refuses an input that git does not track or that has uncommitted
changes, asking git in the input file's own repository, so every rendered entry
comes from a reviewed commit. `--allow-unversioned-input` is the one escape, for
fixtures, the example and tests; the plan then records no commit. Review the
input as code, including selector design, as you would a firewall rule change.
`registration-plan.json` (review only, never runtime authority) records the
input's SHA-256 and commit as `input` and the SHA-256 of the exact
`registrations.json` bytes as `registrations_sha256`; that digest is what the
reviewer approves and what `reconcile.py` checks before it applies.

The output directory must not already exist. Retain `release-verification.json`
with the reviewed deployment receipt. Inspect `manifests.json`, the SPIRE
configurations, IAM policies, registrations and supervisor patches together.
Rendering grants no cluster or cloud authority and does not deploy anything.

The server sets `default_jwt_svid_ttl` to five minutes explicitly; the profile
issues X.509 SVIDs and configures no JWT audience policy. The self-rooted CA
lifetime is `ca_ttl` of 24 hours, and the tests refuse a longer value without a
reviewed rotation plan. The AWS KMS plugin rotates the signing keys; before
enablement, observe at least one complete CA rotation, confirm every consumer
accepts the bundle across it, and record the evidence.

The CA signing keys are held in KMS. `assurance_level` is `standard` or
`regulated`. A `regulated` profile is refused unless `upstream_authority` is set;
a `standard` profile may leave it `null` and stays self-rooted. When set, the
server renders the SPIRE
[`aws_pca` UpstreamAuthority](https://github.com/spiffe/spire/blob/v1.16.0/doc/plugin_server_upstreamauthority_aws_pca.md)
with `region`, `certificate_authority_arn`, `ca_signing_template_arn` and
`signing_algorithm`, and the server role policy gains the three `acm-pca` actions
the plugin documents (`DescribeCertificateAuthority`, `IssueCertificate`,
`GetCertificate`), scoped to that one private CA. The CA must live in the
deployment account and region, sign with the
`SubordinateCACertificate_PathLen0/V1` template, and use `SHA256WITHECDSA` or
`SHA384WITHECDSA`. The `disk` upstream authority keeps CA keys on disk and is
refused, as is any type other than `aws_pca`; `gcp_cas` is not rendered because
this profile runs against AWS identities only. The renderer cannot prove the
private CA root is enterprise-owned. Hardware protection of its key is checked
from evidence: save the output of `aws acm-pca describe-certificate-authority
--certificate-authority-arn <arn>` and run the verifier:

```sh
python3 devops/kubernetes/spire/upstream_ca_evidence.py \
  --config deployment.json --describe describe-certificate-authority.json
```

It exits 1 unless the ARN equals the one in the deployment input, the CA is
`ACTIVE`, its key is elliptic curve to match the ECDSA signing algorithm, and
`KeyStorageSecurityStandard` is `FIPS_140_2_LEVEL_3_OR_HIGHER` (or the equivalent
`CCPC_LEVEL_1_OR_HIGHER`) under the `regulated` assurance level; a `standard`
level accepts `FIPS_140_2_LEVEL_2_OR_HIGHER` too. Keep the saved output and the
verifier result with the deployment receipt. The CA, the KMS keys, the
datastore and the admin identities of the server are tier-zero assets and need
privileged-access controls of their own, which no render output proves. The
rendered `<namespace>-identity-operator-access` admission policy refuses CONNECT
on `pods/exec`, `pods/attach` and `pods/portforward`, and UPDATE on
`pods/ephemeralcontainers`, in the server and agent namespaces unless the requester
is in a group listed in the required `server_operator_groups` input. The operator
who runs `reconcile.py` through `kubectl exec` must be in one of those groups, and
the group must come from your identity provider rather than a name a user can
choose. The policy cannot stop a principal who can edit the policy, the cluster
or the node, and it does not cover the datastore, the KMS keys or the CA.
The server role's IAM trust, permission and KMS key policies are checked from
evidence the same way: save `aws iam get-role`, `get-role-policy`,
`list-role-policies`, `list-attached-role-policies` and one `aws kms get-key-policy`
per signing key, and run `iam_evidence.py` (the arguments are in the README of
`devops/kubernetes/spire`). It exits 1 on any difference from the rendered
documents, including an extra principal or action, a second inline policy or an
attached managed policy. Permission boundaries and service control policies are
not in that output. Add the ACM PCA endpoint to `aws_endpoint_cidrs` so the
server network policy permits it. ACM PCA cannot rotate a CA key under the same
ARN, so changing the upstream CA means restarting the servers.

The consumer caps an X.509 SVID's complete `NotAfter - NotBefore` validity at
3600 seconds. Pinned SPIRE 1.16.0
[backdates `NotBefore` by 10 seconds](https://github.com/spiffe/spire/blob/872c1d55b4d5ac7fc1bf05f0760f2ca740ab37aa/pkg/server/credtemplate/builder.go#L44-L47)
while [setting `NotAfter` to issuance time plus the requested TTL](https://github.com/spiffe/spire/blob/872c1d55b4d5ac7fc1bf05f0760f2ca740ab37aa/pkg/server/credtemplate/builder.go#L545-L554),
subject to an issuer expiration cap. The profile requests `3590s` for the server
default and agent TTLs and 3590 seconds in every registration entry, keeping total
validity within the consumer ceiling. A requested TTL of 3600 seconds could
produce 3610 seconds of validity and be refused. Recheck the pinned issuer's
backdating behavior when upgrading SPIRE.

Validate the exact generated `server.conf` and `agent.conf` with their pinned
SPIRE binaries using `validate -config`. The server also requires `-expandEnv`
and its nonsecret `SPIRE_SERVER_ID`. Validate the manifests against Kubernetes
1.37 schemas with no skipped kinds, then perform API-server dry-run validation
in the selected cluster. Check the admission policy's type-checking status
before admitting workloads. Local CEL tests cover expression behavior; they
cannot prove the cluster installed or enforced the policy.

Inventory every existing and controller-managed pod before installing the
fail-closed admission policy. Its process baseline applies cluster-wide:
nonroot identities, a read-only root filesystem, no added Linux capabilities,
no privilege escalation, RuntimeDefault seccomp, and no host or shared process
namespaces. The exact generated SPIRE agent profile is the only host-privilege
exception. A namespace label or service account alone cannot grant that exception.
Privileged CNI, CSI, monitoring and other system components are part of the
trusted infrastructure boundary. Their current pods do not make their
replacement pods admissible. Before rollout, develop separately reviewed and
tested exact infrastructure profiles and prove that node replacement, storage,
networking and controller recovery still work. Do not add namespace-wide bypasses
or relax the workload baseline to make an infrastructure rollout pass.

Admission does not evict existing pods. Remove or replace any pre-existing
pod that can reach node identity state, control-plane credentials or host
processes outside an approved infrastructure profile before enabling issuance.
Protect PV provisioning, storage classes, CSI drivers and node filesystems:
admitting a PVC or CSI volume does not inspect its backing storage or prove it
cannot expose host state. Prove these independent storage controls as part of
the same acceptance gate.

Never add `insecure_bootstrap` or a routine `join_token` to either
configuration. Tests fail if the rendered output contains them, or any other key
on the forbidden list such as `admin_ids`, `federates_with` or `plugin_cmd`. The
agent attestor renders `skip_kubelet_verification` and
`use_anonymous_authentication` as `false`, and tests pin both values; never
change either to `true`.

Install and test the policies before starting trusted supervisors. Keep pod
and workload-controller mutation, exec and attach, ephemeral-container injection,
service-account token creation, node binding and namespace policy changes
restricted to the appropriate operators. Protect the referenced ConfigMaps,
Secrets, admission policies and bindings with the same change-control boundary.
Workload users cannot administer the issuer or the dedicated supervisor
namespaces. Check both direct `spec.nodeName` selection and the `pods/binding`
and legacy `bindings` APIs: issuer, agent and supervisor namespaces may select
only their configured node-name allowlists. Protect node labels and node-pool
membership independently.

The agent holds `get` on `nodes/proxy`, limited to the configured node names.
Workload attestation needs it to verify pods against the kubelet's secure port
(10250) with the pinned kubelet CA; this is the agent's only kubelet access.
The agent's disk KeyManager keeps only that agent's own node key under its data
directory. The server's signing keys never leave KMS and the server renders no
disk KeyManager.

The server runs as a nonroot process with a read-only root filesystem, private
administrative socket, projected API and AWS tokens, and retained per-replica
storage. The StatefulSet requires separate control-plane nodes and availability
zone spread. Its KMS instance identity includes the recovery epoch and stable
pod ordinal. All replicas use the same supported tag-discovery mode.

### Apply order

The server renders with `disable_migration: true`, so nothing in the running
server creates or upgrades the schema. The renderer emits a one-shot Job named
`spire-server-migrate-<ten hex characters>`, a hash of the whole Job spec and
its `migrate.conf`, with the same server
image, the same database TLS and the same NetworkPolicies and admission policy.
It attests nothing and uses a throwaway in-memory key manager and a disposable
trust domain. Apply the Job, wait for it to report `condition=complete`, and only
then apply the server StatefulSet and the agent DaemonSet. Kubernetes refuses
changes to a Job's pod template, which is why any change to the rendered Job,
such as a new image, node list or database secret, gives it a new name. Delete
the completed Job from the previous render after the new one completes. Coordinate the Job with
the fenced maintenance procedure in the database prerequisite. The Job leaves a bundle for its disposable trust domain in the datastore, and
the real servers report it as a stored federated bundle. The first
`reconcile.py` apply removes it, and `--check` reports any stored foreign bundle
until it is gone (see [Reconcile and retire entries](#reconcile-and-retire-entries)).
Create the
`monitoring` namespace before applying the alert rules ConfigMap.

## Connect only the trusted supervisor

Supervisor patches deliberately leave application readiness and profile
enablement gated. A listening TCP socket does not establish a current approved
identity, registration, policy or usable durable audit. Require the runtime's
actual health signal for that complete join, including failure on withdrawal
or audit degradation, before routing production traffic.

The egress binary supplies `GET /readyz`, with a loopback listener at
`127.0.0.1:8081` by default. For a Kubernetes HTTP readiness probe, bind it to
the pod address and restrict access through the deployment's management-network
policy. For an IPv4 pod network, add this to the egress container:

```yaml
env:
  - name: POD_IP
    valueFrom:
      fieldRef:
        fieldPath: status.podIP
  - name: TROGON_EGRESS_READINESS_LISTEN
    value: "$(POD_IP):8081"
readinessProbe:
  httpGet:
    path: /readyz
    port: 8081
  periodSeconds: 10
  timeoutSeconds: 3
  failureThreshold: 1
```

Use a bracketed IPv6 socket address for an IPv6 probe listener.

A successful probe requires a fresh durable platform audit write and current
identity, registration and policy. Use a separate process-liveness signal so
an issuer or audit outage withdraws routing without restarting a healthy
enforcement process. Record the deployment acceptance evidence separately from
this component health result.

The node agent is a trusted host service. It runs as root with host PID access
and `SYS_PTRACE` for process attestation. The admission exception binds its
namespace, service account, immutable image, entrypoint, arguments, environment,
probe commands, required security controls, volume sources, mounts and approved
node placement. Extra containers, lifecycle commands and host mounts are refused.
Its namespace remains a trusted administrative boundary. It has no host
networking, container-runtime socket, static
join token or access to the server's control-plane nodes. Prove the selected
kernel, runtime and security policy allow accurate process attestation with
these permissions before enabling the profile.

Apply the relevant `supervisor-patches.json` fragment to the operator-owned
supervisor pod specification. Only its exact trusted container receives the
read-only socket directory and `SPIFFE_ENDPOINT_SOCKET`. Read-only mounting
prevents changing the directory; it still permits connecting to its Unix socket.
Do not mount the socket, node data directory, service-account token or SVID key
into an untrusted guest, sidecar, init container or ephemeral container.

The admission policy denies ordinary host volumes globally. It therefore
rejects root, ancestor and alternate host paths without relying on pathname
normalization. The only ordinary host-volume exception is the exact socket
directory for a registered supervisor, mounted read-only at the canonical path
by its registered container. Unregistered callers, mutable-image substitutions,
writable socket mounts and untrusted container mounts are refused. The global
process baseline also refuses privileged companions, extra Linux capabilities
and shared host/process namespaces outside the exact SPIRE agent profile.
Supervisor namespaces use a Pod Security exemption only to permit the exact
socket host mount under this stricter policy.

For the egress consumer, configure the explicit workload profile:

- `TROGON_EGRESS_WORKLOAD_API_ENDPOINT`: the absolute Unix Workload API endpoint.
- `TROGON_EGRESS_WORKLOAD_SPIFFE_ID`: the exact governed local SPIFFE ID.
- `TROGON_EGRESS_REGISTRATION_BUCKET` and `TROGON_EGRESS_REGISTRATION_FENCE`:
  the independently provisioned live registration source and recovery fence.
- `TROGON_EGRESS_POLICY_BUCKET` and `TROGON_EGRESS_POLICY_INCARNATION`:
  the live tenant/project policy source and its independent incarnation.
- `TROGON_EGRESS_ENFORCEMENT_MODE` (flag `--enforcement-mode`): `enforce`, the
  default, or `dry_run`. Any other value refuses startup. See
  [Pilot with dry run](#pilot-with-dry-run).

Every accepted mTLS connection logs a `peer_connection_accepted` event with the
full peer SPIFFE ID, the peer address and the credential source (`workload_api`
or `static_file`). A refused certificate logs the SPIFFE ID it claimed together
with the refusal reason. These events are the service dependency record: they show
which identity actually calls the proxy.

A proxy on the static-file listener profile checks its client trust anchors at
startup. An anchor carrying a URI subject alternative name must name exactly this
trust domain, and one naming another trust domain or carrying several URI subject
alternative names refuses startup. An anchor without a URI subject alternative
name is accepted, so the file must still hold only this trust domain's anchors.

Use authenticated TLS NATS for both live sources and platform audit. Do not
combine this profile with legacy egress listener certificate/key/CA options or
a static-policy fallback. NATS's independently provisioned authenticated
bootstrap remains a separate requirement.
The supervisor owns the Workload API client and candidate keys. A usable
candidate still needs validated live registration, durable platform activation
acknowledgement and a final generation check. Issuer signing and native audit
logs do not replace that gate.

## Isolate live authority streams

Use the [registration operations procedure](./operate-workload-registration.md)
for the existing producer and runner. It distinguishes independent approval,
provisioning, publication, emergency denial and fenced recovery.

Provision separate registration and policy buckets for each tenant/project.
A reader's leader request uses
`$JS.API.STREAM.MSG.GET.KV_<bucket>`; the selected `last_by_subj` is in its
request body. Subject ACLs on that API cannot isolate different tenant keys in
one shared bucket. The stream itself must contain only the selected authority.

For each dedicated bucket, the sole configured stream subject is
`$KV.<bucket>.t<hex_utf8(tenant)>.p<hex_utf8(project)>`, with lowercase hex.
Set per-subject history to 1 and global maximum messages to 1. Explicitly choose
`DiscardOld` so a complete replacement can remove the previous snapshot;
`DiscardNew` at that global cap would strand the first value. Refuse mirrors,
source streams and additional subjects. Apply the reader's exact payload,
aggregate byte, lease and retention bounds. Policy leases are at most 325
seconds; registration leases are at most 330 seconds. Verify replacement,
deletion, expiry and recovery through the actual configured stream.

The authority reader needs publish access only to these request APIs for its
dedicated registration and policy streams:

- `$JS.API.STREAM.INFO.KV_<bucket>`
- `$JS.API.STREAM.MSG.GET.KV_<bucket>`

Constrain reply subscription permissions to the caller's actual inbox
configuration. Audit publication needs its separate exact writer/scope subjects;
it does not grant authority publishing or administration. Do not grant the
runtime `$KV.<bucket>.>` publication, stream creation/update/delete, another
scope's message-read API, or the independent publisher's credential.

Provision and validate replicated file-backed streams under the deployment's
HA contract. A local replica count or in-process revision is not a restore
fence. Keep authoritative publishers, stream administration, recovery approvals
and consumer credentials separate. A producer cannot turn stale restored
registration or policy into fresh authority by renewing its timestamp. Prove
replica loss, leader changes, stream recreation and independently advanced
recovery fences before enabling the profile.

## Alert on issuer events

Route these classes to the on-call channel, with the retained audit log as the
evidence. Keep selector values out of broad logs and alert text: they describe
the trusted workload boundary, so the alert names the entry ID or SPIFFE ID.

| Event class | Source in the rendered deployment |
| --- | --- |
| Registration entry create, update or delete | server `audit_log_enabled` JSON audit log |
| Any entry with admin or downstream privilege | same audit log, plus the reconciliation plan refusing it |
| Agent attestation failure or ban | server JSON log and audit log for the `k8s_psat` attestor |
| Repeated workload attestation failure | agent JSON log from the `k8s` and `unix` attestors |
| Federation relationship change | audit log; the profile renders no federation, so any such event is unexpected |

The profile renders these signals:

- Server and agent `telemetry` blocks with the Prometheus collector on port 9988
  (server) and 9989 (agent), declared as `metrics` container ports. Network
  policies admit ingress to those ports only from the pods labelled
  `app.kubernetes.io/name: <monitoring.scraper>` in the `monitoring.namespace`
  namespace, and the agent admission policy pins the agent container ports.
  The endpoint is plain HTTP inside the cluster; mTLS to the exporter is not
  rendered.
- `alert-rules.json`, a Prometheus rule file also shipped as the
  `spire-alert-rules` ConfigMap in the monitoring namespace (create that namespace
  yourself; load the file into the ruler that serves your scraper). Metric names
  come from SPIRE's telemetry documentation: RPC call counters are keyed
  `rpc.<service>.<method>`, scraped as `spire_server_rpc_<service>_<method>`
  with a `status` gRPC code label, and datastore counters as
  `spire_server_datastore_registration_entry_<action>`. The trust domain service
  is keyed `trustdomain.v1.trust_domain`. Every series the rules name was
  observed on the `/metrics` endpoint of a running SPIRE 1.16.0 server and
  agent; repeat that check when the pinned release changes. SPIRE creates a call
  counter on its first increment, so the first event of a kind is a series that
  appears at 1. Each rule therefore adds the `increase` over the window to the
  value of the series that did not exist one window ago, one metric name at a
  time, and `promtool test rules` cases (a new series at 1, a flat series, an
  increment, a counter reset) pin it. The audit log stays the complete record.

| Alert | Fires on |
| --- | --- |
| `SpireRegistrationEntryChanged` | any create, update or delete entry RPC |
| `SpireRegistrationEntryDatastoreWrite` | any datastore entry create, update or delete |
| `SpireAgentAttestationFailed` | `attest_agent` calls with a non-`OK` status |
| `SpireAgentBanned` | any `ban_agent` call |
| `SpireJoinTokenCreated` | any `create_join_token` call: agents here attest with `k8s_psat` only, so a join token is unexpected |
| `SpireFederationRelationshipChanged` | any create, update or delete federation relationship RPC |
| `SpireWorkloadAttestationFailing` | more than five failed workload attestations in 10 minutes |
| `SpireWorkloadIdentityDenied` | more than five Workload API SVID requests refused with `PermissionDenied` in 10 minutes: a process asks for an identity no entry grants it |
| `SpireSvidRotationFailed` | a non-`OK` agent re-attestation (`spire_agent_node_attestor_new_svid`) or server SVID rotation (`spire_server_svid_rotate`) |

Entry privilege has no metric. SPIRE reports admin and downstream entries only
through the audit log, so alert on audit log lines that have `type="audit"`,
`service="entry.v1.Entry"` and either `admin=true` or `downstream=true`; those
fields are emitted for `BatchCreateEntry` and `BatchUpdateEntry` requests, and
every audit line carries `service` and `method`. Entry deletion lines carry only `entry_id` (plus
`status`, `status_code`, `status_message` on failure), so correlate deleted
IDs with the reviewed `registration-plan.json`. Other audit log fields are
`spiffe_id`, `parent_id` and `selectors`; do not index `selectors` broadly.
The audit log also covers attestation failures and bans (`status="error"` with
`status_code`) and federation changes (`trust_domain_id`), as a second source.

Alert on the audit line, observed on SPIRE 1.16.0, as `type="audit"`,
`msg="API accessed"`, `service="entry.v1.Entry"`, `method="BatchCreateEntry"` (or
`BatchUpdateEntry`), `status="success"` and `admin=true` or `downstream=true`. An
out-of-band `entry create -admin` produced `"admin":true,"downstream":false`, and
`-downstream` produced `"admin":false,"downstream":true`, each with `spiffe_id`,
`parent_id`, `authorized_as":"local"` and `caller_path`.

A join token is minted with `token generate`; it expires after `-ttl` seconds (the
default is 600) and the server prunes it, so it needs no deletion, but a holder
can attest an agent until then. Agents here attest with `k8s_psat` only, and SPIRE
accepts a join token whatever attestors are configured, so `SpireJoinTokenCreated`
is the detection.

A federation relationship makes the server fetch the endpoint. The rendered INFO
level and JSON format log it under `subsystem_name="bundle_client"` with the
`trust_domain` of the relationship: `msg="Trust domain is now managed"` (info,
with `bundle_endpoint_url` and `bundle_endpoint_profile`), then every minute
`msg="Error updating bundle"` (error, with the `error`) or `msg="Bundle refreshed"`
(info). Alert on any `bundle_client` line, because this profile renders no
federation.

Agents attested with `k8s_psat` re-attest to renew their own SVID, so the agent
side of the rotation alert is the `spire_agent_node_attestor_new_svid` series with
the labels `attestor`, `host` and `status`. `spire_agent_agent_svid_rotate` is
never emitted in this deployment, and no rule or dashboard should depend on it.
The server side is `spire_server_svid_rotate`.

### Measure progress

Each progress number has one source:

| Number | How it is produced |
| --- | --- |
| Share of calls authenticated by an attested SVID | The proxy counter of accepted connections, below |
| Static credentials retired | `credential_inventory.py`: `retired.count` against `inventory_size` in its summary, over the inventory taken at the start under [Rollout and rollback](#rollout-and-rollback) |
| Entries with a single broad selector, target zero | `reconcile.py --check`: the `broad_selectors` list in its plan, which counts live entries with fewer than two selectors |
| Median SVID lifetime | Fixed by the rendered 3590 second TTL; read it from the rendered configuration |
| Rotation failures per week | The recording rule `spire:svid_rotation_failures:increase1w` in `alert-rules.json` |
| Time to revoke | `reconcile.py` logs, for each entry it deletes, `deleted entry <id> at <UTC> x509_svid_ttl_seconds=<n> last_svid_expires_at=<UTC> time_to_revoke_seconds=<n>`, then `time to revoke maximum_seconds=<n> entries=<n>`. The value is the entry's X.509-SVID TTL as the server listed it (3590 seconds as rendered): an exact upper bound, since no SVID is issued for the entry after the delete |

The proxy exports the OpenTelemetry counter `egress_proxy.connections.accepted`
(unit `{connection}`) with the required attribute `egress_proxy.credential_source`,
whose values are `workload_api` and `static_file`. Each accepted connection
carries one request, so the share of calls authenticated by an attested SVID is
the `workload_api` count over the total:

```text
sum(increase(egress_proxy_connections_accepted_total{egress_proxy_credential_source="workload_api"}[7d]))
  / sum(increase(egress_proxy_connections_accepted_total[7d]))
```

The query uses the usual Prometheus translation of the instrument and attribute
names (dots to underscores, `_total` for a counter). The proxy exports the
counter over OTLP through the shared telemetry stack, so the series name depends
on the collector's Prometheus translation settings: a collector configured to
keep dots or drop suffixes produces a different name for the same instrument.

## Rollout and rollback

Move a call path to workload identity in this order:

1. Inventory the static credentials on the call path: one record per call
   path with an owner, the credential kind and where it lives (never its
   value), and the state `static`.
2. Issue SVIDs while the old credential still works, and pilot in parallel with
   it (state `parallel`).
3. Run the proxy in dry run. Save its JSON log and run `peer_id_evidence.py`
   over it: it refuses unless every piloted call path's expected caller SPIFFE
   ID appears in an accepted-connection event and in no would-deny event.
   Record the digest it prints as `peer_id_evidence_sha256` on the call path.
   The log shows what the proxy saw; check the callee's own log for the
   proxy's SPIFFE ID the same way before enforcing anything.
4. Add per-ID authorization and test the denials.
5. Switch the proxy to `enforce` (state `enforced`, with the time it started
   and the evidence digest from step 3; the inventory refuses the state
   without it).
6. Only then revoke the static credential (state `retired`).

The inventory is a JSON file you own, kept in version control;
`credential-inventory.example.json` shows the shape. Validate it against the
rendered registrations and print the counts:

```sh
python3 devops/kubernetes/spire/credential_inventory.py \
  --inventory credential-inventory.json \
  --registrations rendered-identity/registrations.json
```

It prints the inventory size, the count per state, `retired` against the
inventory, `static_paths` and `static_credential_still_live` (paths in `static`
or `parallel`, whose old secret still works). It exits 1, naming the call path
and the rule, when a path past `static` names a caller SPIFFE ID that is not a
registered workload, lacks the SPIFFE ID, timestamp or peer ID evidence digest
its state requires, carries
a timestamp that is not RFC 3339 UTC or lies in the future, carries a field that
could hold a credential value, or is `retired` less than one full CA rotation
(the 24 hour `ca_ttl`) after enforcement started.

Check the pilot's saved proxy log before moving a path to `enforced`:

```sh
python3 devops/kubernetes/spire/peer_id_evidence.py \
  --inventory credential-inventory.json \
  --registrations rendered-identity/registrations.json \
  --proxy-log proxy.jsonl --strict
```

It checks every path in state `parallel`, or each `--path`. It exits 1 naming
the call path when no `peer_connection_accepted` event names its caller SPIFFE
ID, when an `egress_would_deny` event names it, or when a line is not a log
record (a skipped line could be a would-deny event). `--strict` also refuses a
peer ID that no inventory path expects. Keep the accepted log beside the
inventory and pass `--evidence-dir` to `credential_inventory.py`: it then finds
each enforced or retired path's log by digest and checks it again.

### Pilot with dry run

Set `TROGON_EGRESS_ENFORCEMENT_MODE=dry_run` for the pilot. Enforcement is the
default, and dry run is chosen only by naming it; an unknown value refuses
startup. Dry run changes one decision: an authenticated identity that holds no
permit for the requested destination is forwarded instead of refused. The proxy
then emits an `egress_would_deny` warning event, and the audit record carries the
dry run enforcement mode with `would_deny`. Authentication, the SVID checks, the
private address guards and refusals because policy is unavailable are unchanged,
so a caller without a valid SVID is still refused. Read the would-deny events as
the list of permits still missing, add them, and switch back to `enforce` when
the list is empty.

Rollback is an explicit operator action; the system never falls back to the old
credential automatically. To roll back, re-enable the old credential path and
switch the proxy back to dry run. Keep the old path disabled but restorable until
one full CA rotation has passed; the inventory refuses `retired` before that.

## Prove failure and recovery before enablement

Retain receipts for copied application keys, a different tenant or container,
wrong PSAT audience, expired/replayed evidence, missing node labels, forged
selectors, signing-role loss, database loss and issuer failover. Exercise both
new handshakes and established sessions during certificate rotation, bundle
removal, registration withdrawal and audit outage. Prove immediate withdrawal
does not wait for an audit acknowledgement.

Confirm rotation under load and during a SPIRE server outage. Agents serve
cached SVIDs only until they expire, so stop the server for longer than the
rendered 3590 second X.509-SVID TTL while the pilot workloads carry traffic.
Record that workloads keep their identity until expiry, that consumers withdraw
at expiry instead of presenting a stale SVID, that `SpireSvidRotationFailed`
fires during the outage, and that rotation resumes without a restart once the
server returns. When an outage outlasts the agent's own SVID, the agent exits and
must re-attest after it restarts; workloads have already withdrawn at SVID
expiry by then. A SPIRE 1.16.0 agent keeps returning the cached SVID after it
has expired, so the consumer's own expiry check is the control: an expired SVID
must be refused by the workload, never trusted because the agent delivered it.

`test_cluster_live.py` in `devops/kubernetes/spire/` is an opt-in test. It renders
the deployment, applies the output to a fresh local kind cluster with a TLS
PostgreSQL, and deletes the cluster when it ends:

```sh
SPIRE_CLUSTER_LIVE=1 mise exec -- python3 -m unittest discover -v \
  -s devops/kubernetes/spire -p 'test_cluster_live.py'
```

It needs kind, kubectl and Docker. It proves that the manifests are admitted and
the agents attest with `k8s_psat`, that the migration Job completes, that
`reconcile.py` converges through `kubectl exec` including an upgrade from the
single alias, that a pilot pod receives exactly its own X.509-SVID on an allowed
node and none on another, that the shipped rules pass `promtool check rules` and
that every series they use exists after its event, that SVIDs rotate under load
and survive a server outage until expiry, and that the admission policies refuse
violating pods.

It substitutes the AWS KMS key manager and the AWS Private CA upstream authority
with disk plugins, stubs image signature verification, and shortens the agent,
server and one entry TTL. Those parts, and the real TTLs, still need the pilot
cluster.

An epoch in a manifest or alias name is not an independent restore fence. Before
restoring issuer state, deny new activation, advance independently governed
registration and policy fences, reconcile revoked registrations and roots, and
retire old node aliases. Do not reconnect consumers until the restored issuer
and live authority sources agree with the new fence. Prove a restored old
database, copied agent cache or old audit acknowledgement cannot reactivate
authority. Repeat backup, restore and key-compromise exercises against the
actual HA database, KMS policies and workload runtime.

Keep the profile disabled until the acceptance contract has evidence for the
complete boundary, including independent registration publishing, retained
audit, isolation, recovery and the chosen external-provider integrations.

## Upgrade notes

Operator-visible changes of the alignment work, each checked against the code:

- The deployment input requires `assurance_level`, `upstream_authority`
  (`null` only at `standard`), `monitoring`, `api_server_port` and, on every
  registration, `node_names`.
- Node aliases are one per node. The first `reconcile.py` apply after the upgrade
  deletes the old single cluster alias and its workload entries and recreates
  the workload entries under the per-node aliases.
- Workload SPIFFE paths with StatefulSet ordinals, IP-shaped or date-shaped
  segments, long digit runs, hashes (including ReplicaSet suffixes) or node names
  are refused.
- A migration Job must complete before the servers start. The first
  `reconcile.py` apply removes its `migration.invalid` bundle.
- `reconcile.py` apply needs `--approved-digest`, and `--check` exits 3 when the
  server holds an admin or downstream entry (it exited 1 before).
- The deployment input requires `server_operator_groups`, a non-empty list of
  groups. A new admission policy refuses exec, attach, port-forward and ephemeral
  containers in the server and agent namespaces for everyone else, so add your
  operators' group before applying, or `reconcile.py` through `kubectl exec` fails.
- A `SpireJoinTokenCreated` alert is added, and every event alert now also counts
  the first event of a new series.
- `render.py` refuses an input that is not committed in git unless
  `--allow-unversioned-input` is given.
- Static-mode egress refuses a certificate valid for more than 3600 seconds or
  outside the X.509-SVID leaf profile, for its own certificate at startup and for
  each peer, and refuses a client trust anchor file in which an anchor names a
  trust domain other than the configured one. A file mixing anchors of several
  trust domains is therefore refused; an anchor with no URI SAN is not judged.
- The egress proxy binary writes JSON logs and exports OTLP telemetry, and
  accepts `--enforcement-mode` or `TROGON_EGRESS_ENFORCEMENT_MODE`: `enforce`
  (default) or `dry_run`.
- The SPIFFE trust profile contract requires `owner` and caps `rotation_overlap`
  at five refresh intervals (it must also be at least three).
- A SPIFFE ID with only a trust domain and no path is refused in the subject
  binding and attestation contracts.

## Source references

- [Pinned SPIRE release source](https://github.com/spiffe/spire/tree/872c1d55b4d5ac7fc1bf05f0760f2ca740ab37aa)
- [Release signing workflow](https://github.com/spiffe/spire/blob/872c1d55b4d5ac7fc1bf05f0760f2ca740ab37aa/.github/workflows/release_build.yaml)
- [Kubernetes PSAT node attestation](https://github.com/spiffe/spire/blob/872c1d55b4d5ac7fc1bf05f0760f2ca740ab37aa/doc/plugin_server_nodeattestor_k8s_psat.md)
- [Kubernetes workload attestation](https://github.com/spiffe/spire/blob/872c1d55b4d5ac7fc1bf05f0760f2ca740ab37aa/doc/plugin_agent_workloadattestor_k8s.md)
- [AWS KMS key manager](https://github.com/spiffe/spire/blob/872c1d55b4d5ac7fc1bf05f0760f2ca740ab37aa/doc/plugin_server_keymanager_aws_kms.md)
- [SQL datastore](https://github.com/spiffe/spire/blob/872c1d55b4d5ac7fc1bf05f0760f2ca740ab37aa/doc/plugin_server_datastore_sql.md)
