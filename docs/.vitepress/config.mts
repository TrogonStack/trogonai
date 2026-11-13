import { fileURLToPath } from "node:url";
import { defineConfig } from "vitepress";
import {
  readAdrRecords,
  readConnectorRecords,
  readGlossaryRecords,
  readProductAreaRecords,
  toAdrSidebarItem,
  toConnectorSidebarItem,
  toGlossarySidebarGroups,
  toProductNavItem,
} from "./helpers";

const base = process.env.DOCS_BASE ?? "/";

export default async () => {
  const rootDir = fileURLToPath(new URL("..", import.meta.url));
  const adrRecords = await readAdrRecords(rootDir);
  const glossaryRecords = await readGlossaryRecords(rootDir);
  const connectorRecords = await readConnectorRecords(rootDir);
  const productItems = [
    { text: "Overview", link: "/product/" },
    ...(await readProductAreaRecords(rootDir)).map(toProductNavItem),
  ];

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
        { text: "Product", activeMatch: "^/product/", items: productItems },
        { text: "Releases", link: "/releases/" },
        {
          text: "Docs",
          activeMatch: "^/(get-started|how-to|architecture|reference|glossary|adr)/",
          items: [
            { text: "Get started", link: "/get-started/" },
            { text: "Glossary", link: "/glossary/" },
            { text: "Architecture Decisions", link: "/adr/" },
          ],
        },
        { text: "GitHub", link: "https://github.com/TrogonStack/trogonai" },
      ],
      sidebar: {
        "/product/": [
          { text: "Product", items: productItems },
          {
            text: "Connectors",
            items: [
              { text: "All connectors", link: "/product/connectors/" },
              ...connectorRecords.map(toConnectorSidebarItem),
            ],
          },
        ],
        "/releases/": [
          {
            text: "Releases",
            items: [
              { text: "Overview", link: "/releases/" },
              { text: "Release channels", link: "/releases/channels" },
            ],
          },
        ],
        "/adr/": [
          {
            text: "Architecture Decision Records",
            items: [{ text: "ADR Index", link: "/adr/" }, ...adrRecords.map(toAdrSidebarItem)],
          },
        ],
        "/glossary/": [
          {
            text: "Glossary",
            items: [{ text: "Overview", link: "/glossary/" }, ...toGlossarySidebarGroups(glossaryRecords)],
          },
        ],
        "/": [
          {
            text: "Get Started",
            items: [
              { text: "Overview", link: "/get-started/" },
              { text: "Build with Bazel", link: "/how-to/use-bazel" },
            ],
          },
          {
            text: "Agents",
            collapsed: true,
            items: [
              {
                text: "Explanation",
                items: [
                  { text: "Agent Platform", link: "/architecture/agent-platform" },
                  { text: "Workflows", link: "/architecture/workflows" },
                ],
              },
            ],
          },
          {
            text: "ACP",
            collapsed: true,
            items: [
              {
                text: "How-to",
                items: [
                  {
                    text: "Retire the ACP Notifications Stream",
                    link: "/how-to/retire-acp-notifications-stream",
                  },
                ],
              },
              {
                text: "Reference",
                items: [{ text: "ACP Conformance", link: "/architecture/acp-conformance" }],
              },
            ],
          },
          {
            text: "Gateway & Channels",
            collapsed: true,
            items: [
              {
                text: "How-to",
                items: [
                  {
                    text: "Receive Meeting Transcript Notifications",
                    link: "/how-to/receive-meeting-transcript-notifications",
                  },
                ],
              },
              {
                text: "Reference",
                items: [{ text: "Container Images", link: "/reference/container-images" }],
              },
              {
                text: "Explanation",
                items: [
                  {
                    text: "Multi-Channel Agent Routing",
                    link: "/architecture/multi-channel-agent-routing",
                  },
                  {
                    text: "Slack Acknowledgement Responses",
                    link: "/architecture/slack-acknowledgement-responses",
                  },
                ],
              },
            ],
          },
          {
            text: "Sessions",
            collapsed: true,
            items: [
              {
                text: "Reference",
                items: [
                  { text: "Session Aggregate", link: "/architecture/session-aggregate" },
                  { text: "Session Query Contract", link: "/architecture/session-queries" },
                  { text: "Session Pagination", link: "/architecture/session-pagination" },
                  { text: "Session Resume Index", link: "/architecture/session-resume-index" },
                  {
                    text: "Session Projection Freshness",
                    link: "/architecture/session-projection-freshness",
                  },
                  {
                    text: "Session Presentation Caches",
                    link: "/architecture/session-presentation-cache",
                  },
                  { text: "Session Title and Preview", link: "/architecture/session-title-and-preview" },
                  { text: "Session Artifacts", link: "/architecture/session-artifacts" },
                  { text: "Session Structured Diff", link: "/architecture/session-structured-diff" },
                  { text: "Session Terminal Replay", link: "/architecture/session-terminal-replay" },
                  { text: "Session Tool Effects", link: "/architecture/session-tool-effects" },
                  { text: "Session Detached Work", link: "/architecture/session-detached-work" },
                  { text: "Session Provider Faults", link: "/architecture/session-provider-faults" },
                  { text: "Session Crash Boundaries", link: "/architecture/session-crash-boundaries" },
                  { text: "Session Doctor", link: "/architecture/session-doctor" },
                  { text: "Session Maintenance", link: "/architecture/session-maintenance" },
                  { text: "Usage Settlement Ledger", link: "/architecture/usage-settlement-ledger" },
                ],
              },
              {
                text: "Explanation",
                items: [
                  {
                    text: "Session Schema Boundaries",
                    link: "/architecture/session-schema-boundaries",
                  },
                ],
              },
            ],
          },
          {
            text: "Keys & Secrets",
            collapsed: true,
            items: [
              {
                text: "How-to",
                items: [
                  { text: "Bring Your Own AWS KMS Key", link: "/how-to/bring-your-own-aws-kms-key" },
                  {
                    text: "Bring Your Own Google Cloud KMS Key",
                    link: "/how-to/bring-your-own-google-cloud-kms-key",
                  },
                  { text: "Bring Your Own OpenBao", link: "/how-to/bring-your-own-openbao" },
                  { text: "Migrate Between Key Backends", link: "/how-to/migrate-key-backends" },
                  { text: "Handle Unusable Keys", link: "/how-to/handle-unusable-keys" },
                  { text: "Add a Connector Provider", link: "/how-to/add-a-connector-provider" },
                  { text: "Connect a Provider", link: "/how-to/connect-a-provider" },
                  { text: "Integrate Connector HTTP", link: "/how-to/integrate-connector-http" },
                  { text: "Operate Connectors", link: "/how-to/operate-connectors" },
                ],
              },
              {
                text: "Reference",
                items: [
                  { text: "Key States", link: "/architecture/key-states" },
                  {
                    text: "Supplied Credential Material",
                    link: "/reference/supplied-credential-material",
                  },
                  { text: "Connection HTTP Proxy", link: "/reference/connection-http-proxy" },
                  { text: "Connector Resource Parents", link: "/reference/connector-resource-parents" },
                  { text: "Connector Security Profile", link: "/reference/connector-security-profile" },
                  { text: "Workload Identity", link: "/reference/workload-identity" },
                  { text: "Connector Account Identity", link: "/reference/connector-account-identity" },
                  { text: "Connector MCP Authorization", link: "/reference/connector-mcp-authorization" },
                  { text: "Connector Control API", link: "/reference/connector-control-api" },
                  { text: "Connector HTTP Transport", link: "/reference/connector-http-transport" },
                  { text: "Connector Binding Integration", link: "/reference/connector-binding-integration" },
                  { text: "Connector Relay Recovery", link: "/reference/connector-relay-recovery" },
                  { text: "Connector Conformance", link: "/reference/connector-conformance" },
                  { text: "Linear Connector Profile", link: "/reference/connector-linear-profile" },
                ],
              },
              {
                text: "Explanation",
                items: [
                  { text: "Key Custody", link: "/architecture/key-custody" },
                  { text: "Key Management", link: "/architecture/key-management" },
                  { text: "Secret Management", link: "/architecture/secret-management" },
                  { text: "Third-Party Connections", link: "/architecture/third-party-connections" },
                  {
                    text: "Provider Credential Custody",
                    link: "/architecture/provider-credential-custody",
                  },
                ],
              },
            ],
          },
          {
            text: "Decider & Events",
            collapsed: true,
            items: [
              {
                text: "Explanation",
                items: [
                  { text: "Decider Platform", link: "/architecture/decider" },
                  { text: "Event Metadata", link: "/architecture/event-metadata" },
                ],
              },
            ],
          },
          {
            text: "Observability",
            collapsed: true,
            items: [
              {
                text: "Reference",
                items: [
                  {
                    text: "OpenTelemetry Transport Context",
                    link: "/architecture/opentelemetry-transport-context",
                  },
                ],
              },
            ],
          },
          {
            text: "Browse",
            items: [
              { text: "Product", link: "/product/" },
              { text: "Releases", link: "/releases/" },
              { text: "Glossary", link: "/glossary/" },
              { text: "Architecture Decision Records", link: "/adr/" },
            ],
          },
        ],
      },
      socialLinks: [{ icon: "github", link: "https://github.com/TrogonStack/trogonai" }],
      editLink: {
        pattern: "https://github.com/TrogonStack/trogonai/edit/main/docs/:path",
        text: "Edit this page on GitHub",
      },
      footer: {
        message: "Released under the Apache License 2.0.",
        copyright: "Copyright TrogonAI contributors.",
      },
    },
  });
};
