export type Environment = "Provider APIs" | "Infrastructure" | "Embedded & test";

type Provider = {
  kind: string;
  name: string;
  summary: string;
  environment: Environment;
  status?: "Test worker";
} & (
  | { logo: { file?: string; website: string; owner?: string }; icon?: never }
  | { icon: "terminal" | "database"; logo?: never }
);

const providers: Provider[] = [
  {
    kind: "azure_container_apps",
    name: "Azure Container Apps",
    environment: "Provider APIs",
    summary: "Allocate a Dynamic Sessions instance from a pool.",
    logo: { file: "azure-container-apps.svg", website: "https://learn.microsoft.com/en-us/azure/container-apps/overview" },
  },
  {
    kind: "daytona",
    name: "Daytona",
    environment: "Provider APIs",
    summary: "Start a sandbox from a named Daytona snapshot.",
    logo: { file: "daytona.svg", website: "https://www.daytona.io/" },
  },
  {
    kind: "e2b",
    name: "E2B",
    environment: "Provider APIs",
    summary: "Run commands and terminals in a template-based sandbox.",
    logo: { file: "e2b.svg", website: "https://e2b.dev/" },
  },
  {
    kind: "fly",
    name: "Fly.io",
    environment: "Provider APIs",
    summary: "Launch a Fly Machine with optional volume snapshots.",
    logo: { website: "https://docs.fly.io/about/brand", owner: "the Fly.io brand kit" },
  },
  {
    kind: "modal",
    name: "Modal",
    environment: "Provider APIs",
    summary: "Run a Modal Sandbox through a Python SDK bridge.",
    logo: { file: "modal.svg", website: "https://modal.com/" },
  },
  {
    kind: "space",
    name: "SPACE",
    environment: "Provider APIs",
    summary: "Reach a Firecracker microVM through Perplexity’s SPACE API.",
    logo: { file: "space.svg", website: "https://github.com/perplexityai/SPACE", owner: "Perplexity SPACE" },
  },
  {
    kind: "substrate",
    name: "Agent Substrate",
    environment: "Provider APIs",
    summary: "Run commands inside an actor created from a digest-pinned image.",
    logo: { file: "substrate.webp", website: "https://ate.dev/" },
  },
  {
    kind: "vercel",
    name: "Vercel",
    environment: "Provider APIs",
    summary: "Lease a Vercel microVM and restore native snapshots.",
    logo: { file: "vercel.svg", website: "https://vercel.com/" },
  },
  {
    kind: "k8s_micro_vm",
    name: "Kubernetes",
    environment: "Infrastructure",
    summary: "Schedule a Job and pod with the cluster’s RuntimeClass.",
    logo: { file: "kubernetes.svg", website: "https://kubernetes.io/" },
  },
  {
    kind: "kube_virt",
    name: "KubeVirt",
    environment: "Infrastructure",
    summary: "Lease a full VM on a KubeVirt cluster.",
    logo: { file: "kubevirt.svg", website: "https://kubevirt.io/" },
  },
  {
    kind: "local_docker",
    name: "Local Docker",
    environment: "Infrastructure",
    summary: "Run a container on the worker's Docker daemon and pause or resume it under the same lease.",
    logo: { file: "docker.svg", website: "https://www.docker.com/", owner: "Docker" },
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
    logo: { file: "firecracker.svg", website: "https://firecracker-microvm.github.io/", owner: "Firecracker" },
  },
  {
    kind: "proxmox",
    name: "Proxmox",
    environment: "Infrastructure",
    summary: "Clone a Proxmox VE template VM for each ephemeral lease.",
    logo: { file: "proxmox.svg", website: "https://www.proxmox.com/" },
  },
  {
    kind: "v8_wasm",
    name: "V8 / Wasmtime",
    environment: "Embedded & test",
    summary: "Evaluate JavaScript or run a Wasm component inside the worker.",
    logo: { file: "v8.svg", website: "https://v8.dev/", owner: "V8" },
  },
  {
    kind: "hyperlight",
    name: "Hyperlight",
    environment: "Embedded & test",
    summary: "Run a Wasm component in a hypervisor-isolated guest on a worker built with the Hyperlight Wasm backend.",
    logo: { file: "hyperlight.svg", website: "https://github.com/hyperlight-dev/hyperlight" },
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
