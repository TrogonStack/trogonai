import * as lucide from "lucide-static";
import { createContentLoader } from "vitepress";
import { z } from "zod";

const Status = z.enum(["available", "preview", "planned"]);

const ProductFrontmatter = z
  .object({
    name: z.string().min(1),
    tagline: z.string().min(1),
    order: z.number().int().nonnegative(),
    status: Status,
    icon: z.string().min(1),
    color: z.string().regex(/^#[0-9a-fA-F]{6}$/),
  })
  .strict();

export type ProductArea = {
  url: string;
  name: string;
  tagline: string;
  order: number;
  status: z.infer<typeof Status>;
  color: string;
  iconStroke: string;
};

declare const data: ProductArea[];
export { data };

export default createContentLoader(["product/*.md", "product/*/index.md"], {
  transform(pages) {
    return pages
      .filter((page) => page.frontmatter.product !== undefined)
      .map((page) => toProductArea(page.url, page.frontmatter.product))
      .sort((left, right) => left.order - right.order);
  },
});

function toProductArea(url: string, raw: unknown): ProductArea {
  const result = ProductFrontmatter.safeParse(raw);

  if (!result.success) {
    throw new Error(`${url}\n\n${z.prettifyError(result.error)}`);
  }

  const { icon, ...product } = result.data;

  return { url, ...product, iconStroke: findIcon(url, icon) };
}

function findIcon(url: string, slug: string) {
  const key = slug
    .split("-")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join("");
  const svg = (lucide as Record<string, string | undefined>)[key];

  if (!svg) {
    throw new Error(`${url}\n\nUnknown lucide icon: ${slug}. Use a name from https://lucide.dev/icons.`);
  }

  return svg.replace(/^[\s\S]*?<svg[^>]*>/, "").replace(/<\/svg>\s*$/, "").trim();
}
