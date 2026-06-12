import { fileURLToPath } from "node:url";
import { defineConfig } from "vitepress";
import { readAdrRecords, toAdrSidebarItem } from "./helpers";

const base = process.env.DOCS_BASE ?? "/";

export default async () => {
  const adrRecords = await readAdrRecords(fileURLToPath(new URL("..", import.meta.url)));

  return defineConfig({
    title: "TrogonAI",
    description: "A distributed agentic platform for coordinating autonomous agents across services and runtimes.",
    lang: "en-US",
    base,
    cleanUrls: true,
    lastUpdated: true,
    ignoreDeadLinks: false,
    head: [["link", { rel: "icon", href: `${base}brand/logo@500x500.png` }]],
    themeConfig: {
      logo: "/brand/logo@500x500.png",
      search: {
        provider: "local",
      },
      nav: [
        { text: "Docs", link: "/get-started/" },
        { text: "ADRs", link: "/adr/" },
        { text: "GitHub", link: "https://github.com/TrogonStack/trogonai" },
      ],
      sidebar: [
        {
          text: "Docs",
          items: [{ text: "Overview", link: "/get-started/" }],
        },
        {
          text: "Architecture",
          items: [{ text: "Event Metadata", link: "/architecture/event-metadata" }],
        },
        {
          text: "Architecture Decision Records",
          items: [{ text: "ADR Index", link: "/adr/" }, ...adrRecords.map(toAdrSidebarItem)],
        },
      ],
      socialLinks: [{ icon: "github", link: "https://github.com/TrogonStack/trogonai" }],
      editLink: {
        pattern: "https://github.com/TrogonStack/trogonai/edit/main/docs/:path",
        text: "Edit this page on GitHub",
      },
      footer: {
        message: "Released under the MIT License.",
        copyright: "Copyright TrogonAI contributors.",
      },
    },
  });
};
