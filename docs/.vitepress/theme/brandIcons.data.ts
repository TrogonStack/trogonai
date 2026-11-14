import { siGooglecloud, siOpenbao } from "simple-icons";

// Resolved at build time so simple-icons never reaches the client bundle.
const ICONS = { googlecloud: siGooglecloud, openbao: siOpenbao };

export type BrandIcon = { hex: string; path: string };

declare const data: Record<keyof typeof ICONS, BrandIcon>;
export { data };

export default {
  load() {
    return Object.fromEntries(Object.entries(ICONS).map(([slug, icon]) => [slug, { hex: icon.hex, path: icon.path }]));
  },
};
