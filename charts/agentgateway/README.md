# agentgateway Helm chart

Deploys the Trogon MCP/A2A agentgateway binaries. The chart is **bin-pluggable**:
the same templates ship every shipping binary listed in §7.1 of
`PENDING_TODO.md`. Pick the binary via `image.bin` (default
`trogon-mcp-gateway`); pre-baked overlays live next to this file as
`values-<binary>.yaml`.

## Install

```sh
helm install mcp-gateway charts/agentgateway
helm install a2a-gateway charts/agentgateway -f charts/agentgateway/values-a2a-gateway.yaml
helm install sts         charts/agentgateway -f charts/agentgateway/values-trogon-sts.yaml
```

## existingSecret schema

The chart projects a pre-existing Kubernetes Secret via `envFrom`. Wire
sensitive material (JWT signing keys, NATS user creds, SpiceDB tokens)
through this Secret — never through `env` or `extraEnv` which become a
ConfigMap.

Reference shape (keys are projected as environment variables):

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: agentgateway-secrets
  namespace: trogon-system
type: Opaque
stringData:
  # SpiceDB preshared key (token mode).
  MCP_GATEWAY_SPICEDB_TOKEN: "REPLACE"

  # NATS credentials (NSC `.creds` file pasted as a string).
  NATS_CREDS: |
    -----BEGIN NATS USER JWT-----
    REPLACE
    ------END NATS USER JWT------
    -----BEGIN USER NKEY SEED-----
    REPLACE
    ------END USER NKEY SEED------

  # JWT signing key — required by trogon-sts, trogon-jwks-publisher,
  # a2a-auth-callout. See devops/nsc/README.md for the Secret shape.
  TROGON_ACCOUNT_SIGNING_KEY: "SAA…REPLACE…"
```

Then set `existingSecret: agentgateway-secrets` in `values.yaml`.

## Multi-binary topology

Every binary in `PENDING_TODO.md` §7.1 is shipped from the same Docker
image — the binary name is the only difference. Use one Helm release
per binary; the overlays in `values-<binary>.yaml` set `image.bin` and
the binary-specific env. Override `existingSecret`, `service`, and
resource limits per release as appropriate.

## NetworkPolicy + PDB

Both are opt-in:

```yaml
networkPolicy:
  enabled: true
  allowedIngressNamespaceSelectors:
    - kubernetes.io/metadata.name: trogon-clients
pdb:
  enabled: true
  minAvailable: 1
```

## NATS provisioning pre-install hook

`natsProvisioner.enabled: true` adds a pre-install `Job` that runs
`nats stream add` / `nats kv add` against the configured cluster.
JSON manifests live in `devops/nats/{streams,kv}/`. Mount the tree
into the Job via an `extraVolumes` PVC or bake it into the
`natsProvisioner.image` — the chart does not bundle JSON dynamically.

## helm test

```sh
helm test mcp-gateway
```

Probes `/readyz` from inside the cluster.

## CRDs

CRDs ship as a separate chart at `charts/agentgateway-crds/` (v0.1
ships an empty stub; see ADR 0017 for the v0.2 Gateway-API plan).
