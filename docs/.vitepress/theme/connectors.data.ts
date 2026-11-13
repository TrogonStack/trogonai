import * as simpleIcons from "simple-icons";
import { createContentLoader } from "vitepress";
import { z } from "zod";
import { EXTRA_BRAND_ICONS } from "./extraBrandIcons";

const CapabilityStatus = z.enum(["available", "preview", "planned"]);

const Capabilities = z
  .object({
    events: CapabilityStatus.optional(),
    actions: CapabilityStatus.optional(),
    channel: CapabilityStatus.optional(),
  })
  .strict()
  .refine((capabilities) => Object.keys(capabilities).length > 0, "Declare at least one capability.");

const ConnectorFrontmatter = z
  .object({
    name: z.string().min(1),
    summary: z.string().min(1),
    category: z.enum(["Communication", "Developer tools", "Productivity", "Observability", "Finance", "Cloud"]),
    icon: z.string().min(1).optional(),
    color: z
      .string()
      .regex(/^#[0-9a-fA-F]{6}$/)
      .optional(),
    capabilities: Capabilities,
  })
  .strict()
  .refine((connector) => connector.icon || connector.color, "Set `icon` (a simple-icons slug) or `color`.");

export type CapabilityStatus = z.infer<typeof CapabilityStatus>;
export type CapabilityKind = keyof z.infer<typeof Capabilities>;

export type Connector = {
  url: string;
  name: string;
  summary: string;
  category: string;
  color: string;
  iconPath: string | null;
  status: CapabilityStatus;
  capabilities: Array<{ kind: CapabilityKind; status: CapabilityStatus }>;
};

declare const data: Connector[];
export { data };

const STATUS_RANK: Record<CapabilityStatus, number> = { available: 0, preview: 1, planned: 2 };

export default createContentLoader("product/connectors/*.md", {
  transform(pages) {
    return pages
      .filter((page) => page.frontmatter.connector !== undefined)
      .map((page) => toConnector(page.url, page.frontmatter.connector))
      .sort((left, right) => left.name.localeCompare(right.name));
  },
});

export function toConnector(url: string, raw: unknown): Connector {
  const result = ConnectorFrontmatter.safeParse(raw);

  if (!result.success) {
    throw new Error(`${url}\n\n${z.prettifyError(result.error)}`);
  }

  const connector = result.data;
  const icon = connector.icon ? findIcon(url, connector.icon) : null;
  const capabilities = (Object.entries(connector.capabilities) as Array<[CapabilityKind, CapabilityStatus]>).map(
    ([kind, status]) => ({ kind, status }),
  );
  const status = capabilities.map((capability) => capability.status).sort(compareStatus)[0];

  return {
    url,
    name: connector.name,
    summary: connector.summary,
    category: connector.category,
    color: connector.color ?? `#${icon?.hex}`,
    iconPath: icon?.path ?? null,
    status,
    capabilities,
  };
}

function findIcon(url: string, slug: string) {
  const key = `si${slug.charAt(0).toUpperCase()}${slug.slice(1)}`;
  const icon = (simpleIcons as Record<string, { hex: string; path: string } | undefined>)[key] ?? EXTRA_BRAND_ICONS[slug];

  if (!icon) {
    throw new Error(`${url}\n\nUnknown simple-icons slug: ${slug}. Use a slug from https://simpleicons.org or set \`color\` instead.`);
  }

  return icon;
}

function compareStatus(left: CapabilityStatus, right: CapabilityStatus) {
  return STATUS_RANK[left] - STATUS_RANK[right];
}
