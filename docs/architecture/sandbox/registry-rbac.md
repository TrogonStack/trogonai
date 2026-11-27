# Tenant-scoped snapshot registry RBAC

This page explains how the Zot snapshot store enforces tenant isolation. Only the platform identity may push artifacts. Tenants may pull only their own repository. Anonymous access is denied.

The configuration lives in the Zot Helm subchart, `devops/helm/charts/zot/`. The chart renders it into `http.accessControl` in the Zot configuration, and also emits a standalone reference ConfigMap from `templates/access-control-configmap.yaml`.

## Trust model

| Actor | Identity | Allowed actions | Repository scope |
|-------|----------|-----------------|------------------|
| Snapshot builder | `sandbox-snapshot-builder` (platform service account, Zot user) | read, create, update (push) | `snapshots/**` |
| Tenant worker | the tenant's `serviceAccount` from values | read (pull) | `snapshots/<tenant id>` |
| Anonymous | none | denied | none |

Digest-pinned restores still flow through workers. Tenants never receive push credentials for the snapshot store. Base image pulls use separate pull secrets, described in [Add a pull secret](../../how-to/sandbox/add-a-pull-secret.md).

## Helm values contract

In the umbrella chart the settings sit under the `zot` key:

```yaml
zot:
  accessControl:
    platformIdentity:
      serviceAccount: sandbox-snapshot-builder
      pushRepositories: ["snapshots/**"]
    tenants:
      - tenantId: acme
        serviceAccount: acme-snapshot-reader
```

Add one `tenants` entry per tenant. The template expands each entry into a Zot repository policy on `snapshots/<tenantId>` with the `read` action only and an empty default policy.

The repository prefix `snapshots` comes from `repositoryLayout.conventions.snapshotPrefix`. A tenant id that contains `:` or `/` is stored under `snapshots/tenant-hex/<hex of the tenant id>`, and the `tenantId` key in `accessControl.tenants` must be that repository segment.

## Operator checklist

1. Enable Zot authentication, with API keys or OIDC. The policies require authenticated identities.
2. Map Kubernetes service accounts to Zot usernames, or to API key identities, for the snapshot builder and each tenant worker.
3. Set `accessControl.tenants` before you grant tenant workers in-cluster registry access.
4. Leave `replication.enabled: false` for single-region installs.

## See also

- [Snapshot store](./snapshot-store.md)
- [Media types](../../reference/sandbox/media-types.md)
- [Deploy the sandbox stack](../../how-to/sandbox/deploy.md)
- [Add a pull secret](../../how-to/sandbox/add-a-pull-secret.md)
- [Alerts](../../reference/sandbox/alerts.md)
