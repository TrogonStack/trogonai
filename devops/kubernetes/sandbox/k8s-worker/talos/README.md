# k8s worker on Talos Linux

Run sandbox pods on Talos nodes under Kata Containers (microVM) or gVisor
(user-space kernel).

## Install the system extensions

Add both extensions to the MachineConfig of every sandbox node, through an
Image Factory schematic or `talosctl upgrade` with an image that includes
them:

- `siderolabs/kata-containers` registers the `kata` (Cloud Hypervisor) and
  `kata-qemu` runtime handlers.
- `siderolabs/gvisor` registers the `runsc` runtime handler.

Kata needs hardware virtualization on the node, so use bare metal or a VM
with nested virtualization enabled.

gVisor needs user namespaces, which Talos turns off by default. Raise the
limit on gVisor nodes with a machine config patch:

```yaml
machine:
  sysctls:
    user.max_user_namespaces: "11255"
```

## Label the nodes

Label each node for the runtimes it carries. A node can carry both:

```sh
kubectl label node <node> sandbox.trogonai.io/kata=true
kubectl label node <node> sandbox.trogonai.io/gvisor=true
```

## Apply the RuntimeClasses

```sh
kubectl apply -f devops/kubernetes/sandbox/k8s-worker/talos/runtimeclass.yaml
```

## Configure the worker

The worker picks a RuntimeClass per isolation tier. A tier with no class of
its own uses `SANDBOX_POD_RUNTIME_CLASS`:

```sh
SANDBOX_POD_RUNTIME_CLASS=kata
SANDBOX_POD_RUNTIME_CLASS_MICRO_VM=kata
SANDBOX_POD_RUNTIME_CLASS_USER_SPACE_KERNEL=gvisor
```

The `trogon-sandbox-k8s-worker` chart sets `SANDBOX_POD_RUNTIME_CLASS` from its
`runtimeClass` value, whose default `kata-fc` does not exist on Talos, so
install it with `--set runtimeClass=kata`. The per-tier variables are not
chart values; set them on the worker Deployment if you need a separate
gVisor tier.
