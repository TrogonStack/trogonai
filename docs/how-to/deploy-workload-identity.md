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
  starting the HA servers. Runtime manifests disable automatic migration.
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
  allow only the configured endpoint CIDRs and ports, plus cluster DNS. Prove
  the selected CNI enforces these policies, including service translation.
- Dedicated server nodes and separate workload-agent nodes across availability
  zones. Apply the configured NodeRestriction-protected pool labels through
  node administration, not a workload or kubelet identity. The configured node
  name allowlists must match those nodes.
- Retained issuer audit collection and monitoring. The server enables native
  JSON audit logs. Collection, retention, access control and alerting still need
  actual deployment evidence; logs alone do not implement platform activation
  acknowledgement.

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
governed recovery state.

Each registration explicitly maps its transport SPIFFE ID to tenant, project,
role and, for an agent runtime, its canonical Ed25519 `did:key` application
identity. Service roles use a null application identity. An agent registration
targets its trusted host adapter; the guest never mounts the endpoint.
The mapping is governed state; the renderer does
not infer authority from a requester-selected URI path. Use the actual admitted
application identity rather than the example alias.

The node alias is constrained by the PSAT cluster, exact agent namespace and
service account, and the protected node-pool label. Workload entries require
the exact workload namespace, service account, container name, container image
and kernel UID together. They have a one-hour X.509 ceiling, no administrative
privilege, no downstream signing and no federation entry. Confirm the image
selector matches the immutable image identity actually reported by the kubelet;
an index digest and a platform manifest digest are not interchangeable labels.

`registrations.json` uses SPIRE's `entry create -data` format. Publish reviewed
entries through the protected administrative socket and a privileged operator
workflow. Neither agent nor supervisor service accounts receive registration
administration permissions. Updating existing entries and removing withdrawn
entries require governed reconciliation; repeatedly creating a new file is not
that control plane.

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

The output directory must not already exist. Retain `release-verification.json`
with the reviewed deployment receipt. Inspect `manifests.json`, the SPIRE
configurations, IAM policies, registrations and supervisor patches together.
Rendering grants no cluster or cloud authority and does not deploy anything.

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

The server runs as a nonroot process with a read-only root filesystem, private
administrative socket, projected API and AWS tokens, and retained per-replica
storage. The StatefulSet requires separate control-plane nodes and availability
zone spread. Its KMS instance identity includes the recovery epoch and stable
pod ordinal. All replicas use the same supported tag-discovery mode.

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

## Prove failure and recovery before enablement

Retain receipts for copied application keys, a different tenant or container,
wrong PSAT audience, expired/replayed evidence, missing node labels, forged
selectors, signing-role loss, database loss and issuer failover. Exercise both
new handshakes and established sessions during certificate rotation, bundle
removal, registration withdrawal and audit outage. Prove immediate withdrawal
does not wait for an audit acknowledgement.

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

## Source references

- [Pinned SPIRE release source](https://github.com/spiffe/spire/tree/872c1d55b4d5ac7fc1bf05f0760f2ca740ab37aa)
- [Release signing workflow](https://github.com/spiffe/spire/blob/872c1d55b4d5ac7fc1bf05f0760f2ca740ab37aa/.github/workflows/release_build.yaml)
- [Kubernetes PSAT node attestation](https://github.com/spiffe/spire/blob/872c1d55b4d5ac7fc1bf05f0760f2ca740ab37aa/doc/plugin_server_nodeattestor_k8s_psat.md)
- [Kubernetes workload attestation](https://github.com/spiffe/spire/blob/872c1d55b4d5ac7fc1bf05f0760f2ca740ab37aa/doc/plugin_agent_workloadattestor_k8s.md)
- [AWS KMS key manager](https://github.com/spiffe/spire/blob/872c1d55b4d5ac7fc1bf05f0760f2ca740ab37aa/doc/plugin_server_keymanager_aws_kms.md)
- [SQL datastore](https://github.com/spiffe/spire/blob/872c1d55b4d5ac7fc1bf05f0760f2ca740ab37aa/doc/plugin_server_datastore_sql.md)
