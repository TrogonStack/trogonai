import { defineConfig } from "vitepress";

const base = process.env.DOCS_BASE ?? "/";

export default defineConfig({
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
      { text: "GitHub", link: "https://github.com/TrogonStack/trogonai" },
    ],
    sidebar: [
      {
        text: "Docs",
        items: [{ text: "Overview", link: "/get-started/" }],
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
