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
            { text: "Key Custody", link: "/architecture/key-custody" },
            { text: "Key Management", link: "/architecture/key-management" },
            { text: "Key States", link: "/architecture/key-states" },
            {
              text: "Multi-Channel Agent Routing",
              link: "/architecture/multi-channel-agent-routing",
            },
            {
              text: "OpenTelemetry Transport Context",
              link: "/architecture/opentelemetry-transport-context",
            },
            { text: "Secret Management", link: "/architecture/secret-management" },
            { text: "Session Aggregate", link: "/architecture/session-aggregate" },
            { text: "Session Artifacts", link: "/architecture/session-artifacts" },
            { text: "Session Crash Boundaries", link: "/architecture/session-crash-boundaries" },
            { text: "Session Detached Work", link: "/architecture/session-detached-work" },
            { text: "Session Doctor", link: "/architecture/session-doctor" },
            { text: "Session Maintenance", link: "/architecture/session-maintenance" },
            { text: "Session Pagination", link: "/architecture/session-pagination" },
            {
              text: "Session Presentation Caches",
              link: "/architecture/session-presentation-cache",
            },
            {
              text: "Session Projection Freshness",
              link: "/architecture/session-projection-freshness",
            },
            { text: "Session Provider Faults", link: "/architecture/session-provider-faults" },
            { text: "Session Query Contract", link: "/architecture/session-queries" },
            { text: "Session Resume Index", link: "/architecture/session-resume-index" },
            {
              text: "Session Schema Boundaries",
              link: "/architecture/session-schema-boundaries",
            },
            { text: "Session Structured Diff", link: "/architecture/session-structured-diff" },
            { text: "Session Terminal Replay", link: "/architecture/session-terminal-replay" },
            { text: "Session Title and Preview", link: "/architecture/session-title-and-preview" },
            { text: "Session Tool Effects", link: "/architecture/session-tool-effects" },
            {
              text: "Usage Settlement Ledger",
              link: "/architecture/usage-settlement-ledger",
            },
          ],
        },
        {
          text: "How-to",
          items: [
            { text: "Bring Your Own AWS KMS Key", link: "/how-to/bring-your-own-aws-kms-key" },
            { text: "Bring Your Own Google Cloud KMS Key", link: "/how-to/bring-your-own-google-cloud-kms-key" },
            { text: "Bring Your Own OpenBao", link: "/how-to/bring-your-own-openbao" },
            { text: "Migrate Between Key Backends", link: "/how-to/migrate-key-backends" },
            { text: "Handle Unusable Keys", link: "/how-to/handle-unusable-keys" },
            {
              text: "Retire the ACP Notifications Stream",
              link: "/how-to/retire-acp-notifications-stream",
            },
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
