# Substrate worker validation deployment

An example deployment for checking the substrate worker against an Agent
Substrate cluster. Every `<...>` value is a placeholder for your cluster.
It is a starting point for validation, not a production bundle.

## Before you apply

- Build and push the worker image from
  `devops/docker/images/trogon-sandbox-substrate-worker/` and the
  `trogon-sandbox-actor-agent` image the actors run. Reference both by
  digest: the agent image is part of the actor template key, so a new digest
  builds new templates.
- Label each Substrate `WorkerPool` with `ate.dev/size` set to one of the
  size names in `SANDBOX_SUBSTRATE_SIZE_CLASS` (`small`, `medium` and `large`
  by default). Actors select their pool by that label, and the memory listed
  for the size is what the actor gets.
- Create the `substrate-api-ca` ConfigMap with the CA that signed
  ate-api-server's certificate under key `ca.pem`.
- Create the `substrate-router-tls` Secret with the router CA (`ca.pem`) and
  a client certificate and key the router accepts (`tls.crt`, `tls.key`).
  The worker refuses partial router TLS material, and plaintext needs
  `SANDBOX_SUBSTRATE_ALLOW_INSECURE_ROUTER=true` with no TLS files at all.
- Grant the `sandbox-substrate-worker` service account what ate-api-server
  requires, and set the token `audience` in `deployment.yaml` to the one it
  accepts.

## Apply

```sh
kubectl apply -f devops/kubernetes/sandbox/substrate-worker/validation/namespace.yaml
kubectl apply -f devops/kubernetes/sandbox/substrate-worker/validation/serviceaccount.yaml
kubectl apply -f devops/kubernetes/sandbox/substrate-worker/validation/deployment.yaml
```

## Settings

- `SANDBOX_SUBSTRATE_SANDBOX_CLASS`: `gvisor` or `microvm`, the class a lease
  gets when it does not pin an isolation tier.
- `SANDBOX_SUBSTRATE_SNAPSHOTS_LOCATION`: base prefix for actor snapshots,
  such as `gs://<bucket>/snapshots`. It goes into each actor template as
  the snapshot storage location.
- `SANDBOX_SUBSTRATE_ROUTER_ENDPOINT`: the router's TLS CONNECT listener; the
  value shown is the worker's default.
