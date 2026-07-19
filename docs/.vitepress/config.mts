import { fileURLToPath } from "node:url";
import { defineConfig } from "vitepress";
import { readAdrRecords, readGlossaryRecords, toAdrSidebarItem, toGlossarySidebarGroups } from "./helpers";

const base = process.env.DOCS_BASE ?? "/";

export default async () => {
  const rootDir = fileURLToPath(new URL("..", import.meta.url));
  const adrRecords = await readAdrRecords(rootDir);
  const glossaryRecords = await readGlossaryRecords(rootDir);

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
        { text: "Glossary", link: "/glossary/" },
        { text: "GitHub", link: "https://github.com/TrogonStack/trogonai" },
      ],
      sidebar: [
        {
          text: "Docs",
          items: [{ text: "Overview", link: "/get-started/" }],
        },
        {
          text: "Architecture",
          items: [
            { text: "ACP Conformance", link: "/architecture/acp-conformance" },
            { text: "Decider", link: "/architecture/decider" },
            { text: "Event Metadata", link: "/architecture/event-metadata" },
          ],
        },
        {
          text: "Glossary",
          items: [{ text: "Overview", link: "/glossary/" }, ...toGlossarySidebarGroups(glossaryRecords)],
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
