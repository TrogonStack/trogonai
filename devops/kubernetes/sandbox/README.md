# Sandbox platform manifests

Cluster-side setup the sandbox workers need beyond their Helm charts in
`devops/helm/charts/trogon-sandbox`: RuntimeClasses, node preparation and example
deployments for workers the charts do not cover.

Each worker gets a directory, and each platform it supports gets one below
it: `devops/kubernetes/sandbox/<worker>/<platform>/`. A platform directory
holds the manifests to apply and a README with the node setup and the worker
settings that go with them. Scripts that prepare a host live in
`<worker>/scripts/`.

| Path | What it sets up |
| --- | --- |
| `k8s-worker/talos/` | Kata and gVisor RuntimeClasses on Talos Linux nodes. |
| `k8s-worker/eks/` | Kata RuntimeClasses on EKS bare-metal node groups. |
| `k8s-worker/aks/` | AKS Pod Sandboxing, which provides its own RuntimeClass. |
| `k8s-worker/gke/` | GKE Sandbox, which provides its own gVisor RuntimeClass. |
| `hyperlight-worker/talos/` | A hyperlight worker on Talos nodes with `/dev/kvm`. |
| `hyperlight-worker/scripts/` | A host check and build for a hyperlight VM. |
| `substrate-worker/validation/` | An example substrate worker deployment. |

The worker images come from `devops/docker/images/trogon-sandbox-<worker>/`.
