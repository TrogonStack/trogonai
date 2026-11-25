import type { CapabilityKind, CapabilityStatus } from "./connectors.data";

export const STATUS_LABELS: Record<CapabilityStatus, string> = {
  available: "Available",
  preview: "Preview",
  planned: "Planned",
};

export const CAPABILITY_LABELS: Record<CapabilityKind, { name: string; description: string }> = {
  events: { name: "Events", description: "The gateway receives this provider's events and publishes them for agents." },
  actions: { name: "Actions", description: "Agents call this provider's API through a project-owned connection." },
  channel: { name: "Channel", description: "Agents hold conversations with people in this provider." },
};
