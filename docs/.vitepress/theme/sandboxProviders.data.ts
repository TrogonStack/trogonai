export type Environment = "Provider APIs" | "Infrastructure" | "Embedded & test";

type Provider = {
  kind: string;
  name: string;
  summary: string;
  environment: Environment;
  status?: "Test worker";
  icon?: "terminal" | "database";
};

const providers: Provider[] = [
  {
    kind: "azure_container_apps",
    name: "Azure Container Apps",
    environment: "Provider APIs",
    summary: "Allocate a Dynamic Sessions instance from a pool.",
  },
  {
    kind: "daytona",
    name: "Daytona",
    environment: "Provider APIs",
    summary: "Start a sandbox from a named Daytona snapshot.",
  },
  {
    kind: "e2b",
    name: "E2B",
    environment: "Provider APIs",
    summary: "Run commands and terminals in a template-based sandbox.",
  },
  {
    kind: "fly",
    name: "Fly.io",
    environment: "Provider APIs",
    summary: "Launch a Fly Machine with optional volume snapshots.",
  },
  {
    kind: "modal",
    name: "Modal",
    environment: "Provider APIs",
    summary: "Run a Modal Sandbox through a Python SDK bridge.",
  },
  {
    kind: "space",
    name: "SPACE",
    environment: "Provider APIs",
    summary: "Reach a Firecracker microVM through the SPACE API.",
  },
  {
    kind: "substrate",
    name: "Agent Substrate",
    environment: "Provider APIs",
    summary: "Run commands inside an actor created from a digest-pinned image.",
  },
  {
    kind: "vercel",
    name: "Vercel",
    environment: "Provider APIs",
    summary: "Lease a Vercel microVM and restore native snapshots.",
  },
  {
    kind: "k8s_micro_vm",
    name: "Kubernetes",
    environment: "Infrastructure",
    summary: "Schedule a Job and pod with the cluster’s RuntimeClass.",
  },
  {
    kind: "kube_virt",
    name: "KubeVirt",
    environment: "Infrastructure",
    summary: "Lease a full VM on a KubeVirt cluster.",
  },
  {
    kind: "local_docker",
    name: "Local Docker",
    environment: "Infrastructure",
    summary: "Run a container on the worker's Docker daemon and pause or resume it under the same lease.",
  },
  {
    kind: "subprocess",
    name: "Subprocess",
    environment: "Infrastructure",
    summary: "Run a lease as a local process using the host’s isolation backend.",
    icon: "terminal",
  },
  {
    kind: "firecracker",
    name: "Direct Firecracker",
    environment: "Infrastructure",
    summary: "Launch a Firecracker microVM for each lease on a Linux host with KVM.",
  },
  {
    kind: "proxmox",
    name: "Proxmox",
    environment: "Infrastructure",
    summary: "Clone a Proxmox VE template VM for each ephemeral lease.",
  },
  {
    kind: "v8_wasm",
    name: "V8 / Wasmtime",
    environment: "Embedded & test",
    summary: "Evaluate JavaScript or run a Wasm component inside the worker.",
  },
  {
    kind: "hyperlight",
    name: "Hyperlight",
    environment: "Embedded & test",
    summary: "Run a Wasm component in a hypervisor-isolated guest on a worker built with the Hyperlight Wasm backend.",
  },
  {
    kind: "mock",
    name: "Memory",
    environment: "Embedded & test",
    status: "Test worker",
    summary: "Exercise the broker with scripted commands and in-memory files.",
    icon: "database",
  },
];

declare const data: Provider[];
export { data };

export default { load: () => providers };
