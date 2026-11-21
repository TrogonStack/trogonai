import { z } from "zod";

export const ProductStatusSchema = z.enum(["available", "preview", "in_progress", "planned"]);

export type ProductStatus = z.infer<typeof ProductStatusSchema>;

export const PRODUCT_STATUS_LABELS: Record<ProductStatus, string> = {
  available: "Available",
  preview: "Preview",
  in_progress: "In progress",
  planned: "Planned",
};
