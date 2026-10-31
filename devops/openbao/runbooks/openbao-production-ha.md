# Runbook: OpenBao Production HA Setup

**State: the production deployment does not exist.** No cluster is
provisioned, no infrastructure-as-code for one is in this repository, and
no gateway configuration points at one. This file records the shape the
deployment must have so that the rest of these runbooks are executable
against it. It is a specification for the operator who builds it, not a
procedure to run today.

**Customer-visible impact when this is wrong:** total. Every credential
resolution that misses the gateway cache reads from OpenBao. An unavailable
OpenBao degrades to "cached credentials keep working until their TTL
expires, then every integration fails closed."

## Required properties

**Integrated storage (Raft), at least three nodes, odd count.** Raft is the
storage backend that keeps state inside the cluster rather than in an
external system with its own availability story. Three nodes tolerate one
loss; five tolerate two. An even count buys nothing.

**Auto-unseal against a cloud KMS key.** Mandatory, not a default.
See [ADR#0052](../../../docs/adr/0052-cloud-kms-production-seal.md) and
[unseal-and-key-custody.md](unseal-and-key-custody.md). The seal stanza is
deployment configuration; no platform code reads it.

**TLS on the listener.** The gateway sends bearer tokens on every request.

**An audit device enabled before the first real secret is written.** OpenBao
refuses to start serving if every enabled audit device fails to log, which
is the correct trade: no audit, no service. See
[audit-log-export-and-review.md](audit-log-export-and-review.md).

**Nodes spread across failure domains.** Availability zones at minimum.
A three-node cluster in one zone has the availability of one zone.

**Snapshots off a follower on a schedule.** See
[backup-and-restore.md](backup-and-restore.md).

## Configuration shape

Confirm every parameter against the OpenBao documentation for the version
you deploy before using this. The stanza names below are the ones OpenBao
inherits from its Vault lineage; the parameter sets have diverged in
places, and this file has not been validated against a running cluster.

```hcl
storage "raft" {
  path    = "/openbao/data"
  node_id = "node-1"
  # one retry_join block per peer
}

listener "tcp" {
  address       = "0.0.0.0:8200"
  tls_cert_file = "/openbao/tls/cert.pem"
  tls_key_file  = "/openbao/tls/key.pem"
}

# One seal stanza. AWS KMS shown; use gcpckms or azurekeyvault as applicable.
seal "awskms" {
  region = "..."
  kms_key_id = "..."
}

api_addr     = "https://openbao-1.internal:8200"
cluster_addr = "https://openbao-1.internal:8201"
```

The KMS key itself has requirements that the stanza does not express:
HSM-backed protection level, KMS-generated key material rather than
imported, IAM-scoped unwrap access limited to the OpenBao service identity,
key-level audit logging on, deletion protection on, rotation deliberately
enabled, and prior key versions retained. All of these are
[ADR#0052](../../../docs/adr/0052-cloud-kms-production-seal.md) section 1
and all of them are load-bearing for recoverability.

## Bring-up order

1. Provision the KMS key and the IAM binding first. The cluster cannot
   initialize without it.
2. Start node 1, initialize, and record the recovery-key shares under the
   custody ceremony in
   [unseal-and-key-custody.md](unseal-and-key-custody.md). With auto-unseal,
   initialization produces recovery keys, not unseal keys.
3. Enable the audit device. Before any secret is written.
4. Join nodes 2 and 3. Confirm `bao operator raft list-peers` shows one
   leader and two voters.
5. Enable the KV v2 mount at `secret/` if the deployment does not have it.
6. Apply the five policies in [../policies](../policies).
7. Bind the policies to an auth method. **This step has no defined
   procedure yet:** the auth-method decision is open, and until it is
   settled the policies exist but nothing assigns them. Do not treat a
   cluster as production-ready with components holding root tokens.
8. Take a snapshot and run the restore drill in
   [backup-and-restore.md](backup-and-restore.md) before any real traffic.
   A backup you have not restored is not a backup.

## Open decisions blocking this

- **Auth method binding.** Which method (AppRole, JWT/OIDC, Kubernetes) and
  how identities map to the five roles.
- **Token TTLs and renewal.** The gateway holds a token for its lifetime
  today; a production token needs a TTL and a renewal path.
- **Namespace or mount isolation between environments.** Whether staging
  and production share a cluster.
