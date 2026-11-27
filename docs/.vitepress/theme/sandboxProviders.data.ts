import type { IconName } from "./icons";

export type Environment = "Provider APIs" | "Infrastructure" | "Embedded & test";

type Provider = {
  kind: string;
  name: string;
  summary: string;
  environment: Environment;
  status?: "Test worker";
  icon: IconName;
  mark?: { src: string; website: string; owner?: string };
};

const providers: Provider[] = [
  {
    kind: "azure_container_apps",
    name: "Azure Container Apps",
    environment: "Provider APIs",
    summary: "Allocate a Dynamic Sessions instance from a pool.",
    icon: "cloud",
    mark: {
      src: "https://raw.githubusercontent.com/Azure/bicep/main/src/vscode-bicep-ui/packages/components/assets/azure-architecture-icons/containers/02884-icon-service-Worker-Container-App.svg",
      website: "https://learn.microsoft.com/en-us/azure/container-apps/overview",
    },
  },
  {
    kind: "daytona",
    name: "Daytona",
    environment: "Provider APIs",
    summary: "Start a sandbox from a named Daytona snapshot.",
    icon: "terminal",
    mark: {
      src: "https://framerusercontent.com/images/k2z1jCEWqymEd7reVoRseEVMFHU.svg",
      website: "https://www.daytona.io/",
    },
  },
  {
    kind: "e2b",
    name: "E2B",
    environment: "Provider APIs",
    summary: "Run commands and terminals in a template-based sandbox.",
    icon: "code",
    mark: {
      src: "https://e2b.dev/brand/e2b-symbol-black-s.svg",
      website: "https://e2b.dev/",
    },
  },
  {
    kind: "fly",
    name: "Fly.io",
    environment: "Provider APIs",
    summary: "Launch a Fly Machine with optional volume snapshots.",
    icon: "globe",
  },
  {
    kind: "modal",
    name: "Modal",
    environment: "Provider APIs",
    summary: "Run a Modal Sandbox through a Python SDK bridge.",
    icon: "boxes",
    mark: {
      src: "https://modal.com/brand/modal-mark.svg",
      website: "https://modal.com/",
    },
  },
  {
    kind: "space",
    name: "SPACE",
    environment: "Provider APIs",
    summary: "Reach a Firecracker microVM through Perplexity’s SPACE API.",
    icon: "cpu",
    mark: {
      src: "https://www.perplexity.ai/favicon.svg",
      website: "https://github.com/perplexityai/SPACE",
      owner: "Perplexity SPACE",
    },
  },
  {
    kind: "substrate",
    name: "Agent Substrate",
    environment: "Provider APIs",
    summary: "Run commands inside an actor created from a digest-pinned image.",
    icon: "bot",
    mark: {
      src: "https://ate.dev/icons/logo_hu_93fe6491255f7836.webp",
      website: "https://ate.dev/",
    },
  },
  {
    kind: "vercel",
    name: "Vercel",
    environment: "Provider APIs",
    summary: "Lease a Vercel microVM and restore native snapshots.",
    icon: "layers",
    mark: {
      src: "https://vercel.com/geist/brands/assets/vercel-symbol-black.svg",
      website: "https://vercel.com/",
    },
  },
  {
    kind: "k8s_micro_vm",
    name: "Kubernetes",
    environment: "Infrastructure",
    summary: "Schedule a Job and pod with the cluster’s RuntimeClass.",
    icon: "network",
    mark: {
      src: "https://raw.githubusercontent.com/cncf/artwork/main/projects/kubernetes/icon/color/kubernetes-icon-color.svg",
      website: "https://kubernetes.io/",
    },
  },
  {
    kind: "kube_virt",
    name: "KubeVirt",
    environment: "Infrastructure",
    summary: "Lease a full VM on a KubeVirt cluster.",
    icon: "server",
    mark: {
      src: "https://raw.githubusercontent.com/cncf/artwork/main/projects/kubevirt/icon/color/kubevirt-icon-color.svg",
      website: "https://kubevirt.io/",
    },
  },
  {
    kind: "local_docker",
    name: "Local Docker",
    environment: "Infrastructure",
    summary: "Run a container on the worker's Docker daemon and pause or resume it under the same lease.",
    icon: "package",
    mark: {
      src: "https://www.docker.com/app/uploads/brand/logo/docker-mark-ether.svg",
      website: "https://www.docker.com/",
      owner: "Docker",
    },
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
    icon: "flame",
    mark: {
      src: "https://raw.githubusercontent.com/firecracker-microvm/firecracker/main/docs/images/fc_logo_icon.svg",
      website: "https://firecracker-microvm.github.io/",
      owner: "Firecracker",
    },
  },
  {
    kind: "proxmox",
    name: "Proxmox",
    environment: "Infrastructure",
    summary: "Clone a Proxmox VE template VM for each ephemeral lease.",
    icon: "server",
    mark: {
      src: "https://www.proxmox.com/favicon.svg",
      website: "https://www.proxmox.com/",
    },
  },
  {
    kind: "v8_wasm",
    name: "V8 / Wasmtime",
    environment: "Embedded & test",
    summary: "Evaluate JavaScript or run a Wasm component inside the worker.",
    icon: "code",
    mark: {
      src: "https://v8.dev/_img/v8.svg",
      website: "https://v8.dev/",
      owner: "V8",
    },
  },
  {
    kind: "hyperlight",
    name: "Hyperlight",
    environment: "Embedded & test",
    summary: "Run a Wasm component in a hypervisor-isolated guest on a worker built with the Hyperlight Wasm backend.",
    icon: "cpu",
    mark: {
      src: "https://raw.githubusercontent.com/cncf/artwork/main/projects/hyperlight/icon/color/hyperlight-icon-color.svg",
      website: "https://github.com/hyperlight-dev/hyperlight",
    },
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
