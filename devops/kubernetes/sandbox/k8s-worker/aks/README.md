# k8s worker on AKS

AKS Pod Sandboxing runs pods in Kata microVMs on Hyper-V isolated node pools
and creates the RuntimeClass for them. This folder has no manifest: AKS
creates and owns the `kata-vm-isolation` RuntimeClass, and applying an
object with the same name fights that reconciliation.

## Create the node pool

Pod Sandboxing needs Azure Linux and a generation 2 VM size that supports
nested virtualization:

```sh
az aks nodepool add \
  --resource-group <resource-group> \
  --cluster-name <cluster> \
  --name sandboxpool \
  --os-sku AzureLinux \
  --workload-runtime KataVmIsolation \
  --node-vm-size <vm-size> \
  --node-count 2
```

## Configure the worker

```sh
SANDBOX_POD_RUNTIME_CLASS=kata-vm-isolation
SANDBOX_POD_RUNTIME_CLASS_MICRO_VM=kata-vm-isolation
```

With the `sandbox-k8s-worker` chart, pass `--set runtimeClass=kata-vm-isolation`.

The managed class does not pin pods to the sandbox pool. If the cluster has
other pools, either create your own RuntimeClass under a different name
whose name contains `kata` (for example `kata-sandboxpool`), with
`handler: kata` and a `scheduling.nodeSelector` of
`kubernetes.azure.com/agentpool: sandboxpool`, and point the worker at it;
or keep the sandbox pool as the only Pod Sandboxing pool.

AKS ships no gVisor. For the user-space kernel tier, run the aca worker
against Azure Container Apps Dynamic Sessions instead.
