# k8s worker on GKE

GKE Sandbox runs pods under gVisor and creates the RuntimeClass for them.
This folder has no manifest: GKE creates and reconciles the `gvisor`
RuntimeClass, and applying an object with the same name conflicts with it.
The managed class already selects and tolerates the sandbox node pool.

## Create the node pool

Enable GKE Sandbox on a dedicated node pool. GKE labels and taints the pool
itself:

```sh
gcloud container node-pools create sandbox-pool \
  --cluster <cluster> \
  --region <region> \
  --machine-type <machine-type> \
  --sandbox type=gvisor
```

## Configure the worker

```sh
SANDBOX_POD_RUNTIME_CLASS=gvisor
SANDBOX_POD_RUNTIME_CLASS_USER_SPACE_KERNEL=gvisor
```

With the `trogon-sandbox-k8s-worker` chart, pass `--set runtimeClass=gvisor`.
Leaving `SANDBOX_POD_RUNTIME_CLASS_MICRO_VM` unset makes microVM-tier leases
fall back to `gvisor` too, so they get user-space kernel isolation rather
than a microVM.

GKE ships no Kata. For microVM or full VM leases, run the kubevirt worker
on a node pool with nested virtualization.
