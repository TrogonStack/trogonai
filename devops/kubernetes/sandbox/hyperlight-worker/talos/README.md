# hyperlight worker on Talos Linux

Run the hyperlight worker on Talos nodes that expose `/dev/kvm`. The pod
mounts the device through a hostPath volume and runs privileged.

The worker binary that `trogon-sandbox-hyperlight-worker` builds ships no
hypervisor backend and exits at startup with "no hypervisor is available on
this host". These manifests are for an image built from a binary that
passes a real backend to `trogon_sandbox_hyperlight_worker::serve`.

## Check the nodes

Confirm each sandbox node has the KVM device:

```sh
talosctl -n <node-ip> read /dev/kvm
```

If the device is missing on a Talos VM, give the VM a host CPU model and
enable nested virtualization on the hypervisor that runs it. Then label the
node:

```sh
kubectl label node <node> sandbox.trogonai.io/kvm=true
```

## Files

- `namespace.yaml`: the `sandbox-hyperlight` namespace, labelled for the
  privileged Pod Security profile the `/dev/kvm` mount needs.
- `pvc.yaml`: the volume holding the Wasm components
  (`SANDBOX_HYPERLIGHT_COMPONENT_ROOT`).
- `configmap.yaml`: the worker environment. Replace the `<nats-host>` and
  `<registry-host>` placeholders.
- `deployment.yaml`: a single replica with `/dev/kvm` and the components
  volume mounted. Replace the `<registry>` and `<tag>` placeholders in the
  image.

## Apply

```sh
kubectl apply -f devops/kubernetes/sandbox/hyperlight-worker/talos/namespace.yaml
kubectl apply -f devops/kubernetes/sandbox/hyperlight-worker/talos/pvc.yaml
kubectl apply -f devops/kubernetes/sandbox/hyperlight-worker/talos/configmap.yaml
kubectl apply -f devops/kubernetes/sandbox/hyperlight-worker/talos/deployment.yaml
```

## Smoke test

```sh
kubectl -n sandbox-hyperlight logs deploy/sandbox-hyperlight-worker
```

A worker with a backend logs `worker ready` with
`lease_subject=sandbox.provider.hyperlight.lease` and takes leases from that
subject in the `sandbox-provider-hyperlight` queue group.
