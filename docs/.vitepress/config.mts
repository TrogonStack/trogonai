import { defineConfig } from "vitepress";

const base = process.env.DOCS_BASE ?? "/";

export default defineConfig({
  title: "TrogonAI",
  description: "A distributed agentic platform for coordinating autonomous agents across services and runtimes.",
  lang: "en-US",
  base,
  cleanUrls: true,
  lastUpdated: true,
  ignoreDeadLinks: true,
  head: [["link", { rel: "icon", href: `${base}brand/logo@500x500.png` }]],
  themeConfig: {
    logo: "/brand/logo@500x500.png",
    search: {
      provider: "local",
    },
    nav: [
      { text: "Docs", link: "/get-started/" },
      { text: "Release notes", link: "/release-notes/v0.1" },
      { text: "GitHub", link: "https://github.com/TrogonStack/trogonai" },
    ],
    sidebar: [
      {
        text: "Get started",
        items: [
          { text: "Overview", link: "/get-started/" },
          { text: "Deploy the agentgateway", link: "/get-started/deploy-agentgateway" },
          { text: "Connect MCP clients", link: "/get-started/connect-mcp-clients" },
        ],
      },
      {
        text: "Identity & gateway",
        items: [
          { text: "Overview", link: "/identity/overview" },
          { text: "Operator overview", link: "/identity/mcp-gateway-operator-overview" },
          { text: "Deploy multi-binary topology", link: "/identity/howto-deploy-multi-binary" },
          { text: "Migrate from raw NATS", link: "/identity/howto-migrate-from-raw-nats" },
          { text: "Environment reference", link: "/identity/reference-env" },
          { text: "Metrics reference", link: "/identity/reference-metrics" },
          { text: "Traces reference", link: "/identity/reference-traces" },
          { text: "CLI reference", link: "/identity/reference-cli" },
          { text: "Subject grammar", link: "/identity/reference-subject-grammar" },
          { text: "Audit envelope", link: "/identity/reference-audit-envelope" },
          { text: "NATS headers", link: "/identity/reference-nats-headers" },
          { text: "Error codes", link: "/identity/reference-error-codes" },
        ],
      },
      {
        text: "A2A",
        items: [
          { text: "Architecture", link: "/a2a/explanation/architecture" },
          { text: "Subject ACL quick ref", link: "/a2a/reference/subject-acl-quickref" },
          { text: "JetStream account streams", link: "/a2a/reference/jetstream-account-streams" },
          { text: "Runtime env", link: "/a2a/reference/runtime-env" },
          { text: "NSC bootstrap", link: "/a2a/how-to/operators/nsc-account-bootstrap" },
          { text: "Push DLQ triage", link: "/a2a/how-to/operators/push-dlq-triage" },
          { text: "Streaming back-pressure", link: "/a2a/how-to/operators/streaming-backpressure" },
        ],
      },
      {
        text: "ACP",
        items: [
          { text: "Overview", link: "/acp/" },
        ],
      },
      {
        text: "Operations",
        items: [
          { text: "Runbook", link: "/runbook/agentgateway" },
          { text: "Threat model", link: "/security/threat-model" },
          { text: "Roadmap (v0.2)", link: "/roadmap/agentgateway-v0.2" },
        ],
      },
      {
        text: "Release",
        items: [
          { text: "v0.1 release notes", link: "/release-notes/v0.1" },
        ],
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
