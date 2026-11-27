# k8s worker on EKS

EKS has no managed pod sandboxing, so run Kata Containers yourself on a
dedicated node group of bare-metal instances.

## Create the node group

Use `*.metal` instance types (for example `c5.metal` or `m6i.metal`).
Standard EC2 instances do not expose hardware virtualization to the guest,
so Kata cannot start microVMs on them.

Label and taint the nodes so only sandbox pods land there, either in the
node group definition or by hand:

```sh
kubectl label node <node> sandbox.trogonai.io/kata=true
kubectl taint node <node> sandbox.trogonai.io/kata=true:NoSchedule
```

## Install Kata

Install Kata with the kata-deploy DaemonSet from the Kata Containers release
that matches your node OS and Kubernetes version, restricted to the labelled
nodes. kata-deploy registers the `kata-qemu` and `kata-fc` runtime handlers.
`kata-fc` (Firecracker) also needs the containerd devmapper snapshotter on
the node; without it, use `kata-qemu`.

kata-deploy can create its own RuntimeClasses. If it does, delete or skip
the ones it creates under these names, since they carry no nodeSelector or
toleration, then apply this folder's:

```sh
kubectl apply -f devops/kubernetes/sandbox/k8s-worker/eks/runtimeclass.yaml
```

## Configure the worker

```sh
SANDBOX_POD_RUNTIME_CLASS=kata-fc
SANDBOX_POD_RUNTIME_CLASS_MICRO_VM=kata-fc
```

The `sandbox-k8s-worker` chart's `runtimeClass` value sets
`SANDBOX_POD_RUNTIME_CLASS` and already defaults to `kata-fc`; use
`--set runtimeClass=kata-qemu` on nodes without devmapper. For the
user-space kernel tier, install gVisor on a node group, add a `gvisor`
RuntimeClass for it and set `SANDBOX_POD_RUNTIME_CLASS_USER_SPACE_KERNEL=gvisor`
on the worker Deployment. For full VMs, use the kubevirt worker.
