# Migrate sandbox identities

New installations use sandbox-owned names for the API-key bucket, the authorization tenant type and Kubernetes worker resources. An existing installation that used other names must select those names before upgrading, then migrate stored credentials, authorization relationships and worker resources separately.

The migration changes configuration and stored identities. It does not move a running sandbox or change an existing lease's tenant, native ID or worker ownership.

## Preserve an existing installation

Back up the API-key bucket and the SpiceDB datastore through the deployment's normal protected backup process. Keep the backups out of source control. Record the current worker image, service account and naming configuration.

Before updating binaries, set these values for the broker to the names your installation already uses:

```text
SANDBOX_API_KEY_BUCKET=<current-api-key-bucket>
SANDBOX_SPICEDB_TENANT_RESOURCE_TYPE=<current-tenant-resource-type>
```

The defaults are `sandbox-api-keys` and `sandbox_tenant`. Set `SANDBOX_API_KEY_BUCKET` to the same bucket for the WebSocket gateway, the SSH gateway and every `trogon-sandbox-admin api-key` command. Confirm which bucket actually contains the deployed keys before selecting an override.

For the Helm chart at `devops/helm/charts/trogon-sandbox`, keep a protected upgrade values file with:

```yaml
global:
  apiKeyBucket: <current-api-key-bucket>
  spicedb:
    tenantResourceType: <current-tenant-resource-type>
trogon-sandbox-k8s-worker:
  resourceNaming: legacy
```

Each broker checks exactly one configured tenant type. A denied check never falls back to another type.

The bundled SpiceDB chart defines only the `sandbox_tenant` schema and refuses to render with another tenant type. If your installation uses another type, set `spicedb.enabled=false` and point `global.spicedb.endpoint` at an operator-managed SpiceDB that holds your schema. Do not replace a shared datastore's schema with the bundled one.

With the memory datastore, a restart loses relationships added after bootstrap. Move to a durable datastore before upgrading. Schema selection alone does not preserve volatile relationship data.

Validate rendered manifests before applying the upgrade. Confirm the API-key bucket on every reader and writer and the broker tenant type. After upgrading, verify a known allowed principal, an unrelated principal that must be denied, and a cross-tenant request that must be denied.

## Migrate API keys

The canonical bucket is `sandbox-api-keys`. Bucket selection is exact: readers do not search another bucket if a key is missing.

Stop API-key minting and revocation while you take a consistent copy. Copy the hashed key names and complete serialized records into the new bucket with audited operator tooling. Preserve the original tenant, service account, creation time and the storage, retention, replication and access settings. Never mint replacement bearer tokens as a substitute for copying records, and never expose bearer tokens in an export or verification log.

Compare the complete copied key set and record contents before switching. Update the broker, gateways and admin commands to `SANDBOX_API_KEY_BUCKET=sandbox-api-keys` together while issuance remains stopped. Verify existing keys and negative controls against every entry point before resuming changes. Keep the old bucket until the rollback window closes.

## Migrate authorization relationships

Keep both tenant definitions in the external SpiceDB while you copy relationships. Preserve tenant object IDs, subjects, subject relations and any relationship restrictions. Apply the copy consistently with respect to writes and deletions, so that a grant revoked during the copy does not reappear.

Verify the copied permissions with both allow and deny controls before selecting `SANDBOX_SPICEDB_TENANT_RESOURCE_TYPE=sandbox_tenant`. For Helm, set `global.spicedb.tenantResourceType=sandbox_tenant`. Only the selected type is authoritative for each broker.

After all consumers use the canonical type and the rollback window closes, retire the old relationships through your normal authorization administration process. Remove the old tenant definition only after no retained relationship refers to it. Keep any other applications' required definitions intact.

## Migrate worker resource names

Before upgrading an existing Kubernetes worker, select `SANDBOX_K8S_RESOURCE_NAMING=legacy` or `--resource-naming legacy`. Retain its service account with `SANDBOX_SERVICE_ACCOUNT`, and use `SANDBOX_IMAGE` to select the guest image you verified with the old workers. For Daytona, keep `SANDBOX_DAYTONA_SNAPSHOT` pinned to a verified snapshot. This keeps resource discovery, clone secret lookup and guest image selection tied to the existing deployment.

For Helm, set `trogon-sandbox-k8s-worker.resourceNaming=legacy` and preserve the installed `trogon-sandbox-k8s-worker.serviceAccountName` and `trogon-sandbox-k8s-worker.image` values. Change `resourceNaming` to `sandbox` only after the drain and verification below.

The two modes stamp different names on the objects the worker creates:

| | `sandbox` (default) | `legacy` |
| --- | --- | --- |
| Managed-by label value | `sandbox-worker` | `legacy-runner` |
| Job, repository and egress labels | `sandbox.trogon.live/{job,repo,egress}` | `legacy/{job,repo,egress}` |
| Agent name | `sandbox-agent` | `legacy-agent` |
| Warm pool name | `sandbox-warm-pool` | `legacy-warm-pool` |
| Secrets bucket | `sandbox-secrets` | `legacy-secrets` |

Workers do not combine secret stores or resource ownership namespaces.

Drain the old worker fleet and confirm that every old Job, warm Pod and tracked native resource has completed its normal cleanup before changing the naming mode, service account or guest image. See [Drain or cordon a worker](./drain-a-worker.md). Keep worker ownership IDs and lease identity intact while draining. Do not relabel live resources to make another worker adopt them. Start the canonical fleet only after its service account, policies, guest image and secret bucket have been provisioned and verified.

## Roll back

Before resuming writes after a cutover, verify that the old bucket, relationships and worker configuration remain available and unchanged. With issuance and relationship writes still stopped, point the updated services back to their recorded selections and repeat the allow, deny and cross-tenant checks.

After writes resume in the canonical stores, switching back without reconciling new keys and revocations can restore revoked access or lose valid credentials. Freeze writes again and reconcile the complete current authorization state before rolling back. Keep both tenant definitions until reconciliation and rollback verification have finished.

For a worker rollback, drain the canonical fleet before returning to the old naming. Apply the same ownership and exact-resource cleanup checks you used for the forward migration.
