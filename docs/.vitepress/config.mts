import { fileURLToPath } from "node:url";
import { defineConfig } from "vitepress";
import {
  readAdrRecords,
  readGlossaryRecords,
  readProductAreaRecords,
  toAdrSidebarItem,
  toGlossarySidebarGroups,
  toProductNavItem,
} from "./helpers";

const base = process.env.DOCS_BASE ?? "/";

export default async () => {
  const rootDir = fileURLToPath(new URL("..", import.meta.url));
  const adrRecords = await readAdrRecords(rootDir);
  const glossaryRecords = await readGlossaryRecords(rootDir);
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
            { text: "Protocols", link: "/architecture/agent-protocols" },
            { text: "Glossary", link: "/glossary/" },
            { text: "Architecture Decisions", link: "/adr/" },
          ],
        },
        { text: "GitHub", link: "https://github.com/TrogonStack/trogonai" },
      ],
      sidebar: {
        "/product/": [{ text: "Product", items: productItems }],
        "/releases/": [
          {
            text: "Releases",
            items: [
              { text: "Overview", link: "/releases/" },
              { text: "Release channels", link: "/releases/channels" },
              { text: "Unreleased", link: "/releases/unreleased" },
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
                  { text: "Sandbox Client", link: "/architecture/sandbox-foundation" },
                ],
              },
            ],
          },
          {
            text: "Sandbox",
            collapsed: true,
            items: [
              {
                text: "Tutorials",
                items: [
                  { text: "Your First Sandbox Lease", link: "/get-started/sandbox/first-sandbox-lease" },
                  { text: "Onboard a Tenant", link: "/get-started/sandbox/onboard-tenant" },
                  { text: "Publish a Signed Snapshot", link: "/get-started/sandbox/publish-signed-snapshot" },
                ],
              },
              {
                text: "How-to",
                items: [
                  { text: "Run the Stack Locally", link: "/how-to/sandbox/run-the-stack-locally" },
                  { text: "Run the Subprocess Worker on macOS", link: "/how-to/sandbox/run-the-subprocess-worker-on-macos" },
                  { text: "Deploy with Helm", link: "/how-to/sandbox/deploy" },
                  { text: "Run the Docker Smoke Test", link: "/how-to/sandbox/docker-smoke" },
                  { text: "Validate on a Linux KVM Host", link: "/how-to/sandbox/validate-on-a-linux-kvm-host" },
                  { text: "Drain or Cordon a Worker", link: "/how-to/sandbox/drain-a-worker" },
                  { text: "Metrics and Traces", link: "/how-to/sandbox/observability" },
                  { text: "Sign Worker Dispatches", link: "/how-to/sandbox/sign-worker-dispatches" },
                  { text: "Migrate Sandbox Identities", link: "/how-to/sandbox/migrate-sandbox-identities" },
                  { text: "Change a Wire Type Safely", link: "/how-to/sandbox/guard-the-wire-contract" },
                  { text: "Measure Sandbox Latency", link: "/how-to/sandbox/measure-latency" },
                  { text: "Firecracker Worker", link: "/how-to/sandbox/firecracker-worker" },
                  { text: "Daytona Worker", link: "/how-to/sandbox/daytona-worker" },
                  { text: "E2B Worker", link: "/how-to/sandbox/e2b-worker" },
                  { text: "Modal Worker", link: "/how-to/sandbox/modal-worker" },
                  { text: "SPACE Worker", link: "/how-to/sandbox/space-worker" },
                  { text: "V8 and Wasm Worker", link: "/how-to/sandbox/v8-wasm-worker" },
                  { text: "Vercel Sandbox Worker", link: "/how-to/sandbox/vercel-worker" },
                  { text: "Agent Substrate Sandbox", link: "/how-to/sandbox/run-a-substrate-sandbox" },
                  { text: "Native Passthrough and Regions", link: "/how-to/sandbox/native-passthrough-and-regions" },
                  { text: "Build an Agent Image", link: "/how-to/sandbox/build-an-agent-image" },
                  { text: "Add a Pull Secret", link: "/how-to/sandbox/add-a-pull-secret" },
                  { text: "Mount Remote Storage", link: "/how-to/sandbox/mount-remote-storage" },
                  { text: "Expose Sandbox Ports", link: "/how-to/sandbox/expose-sandbox-ports" },
                  { text: "Open an Interactive Terminal", link: "/how-to/sandbox/open-an-interactive-terminal" },
                  { text: "Stream Command Output", link: "/how-to/sandbox/stream-command-output" },
                  { text: "Run a Headless Browser", link: "/how-to/sandbox/run-a-headless-browser" },
                  { text: "Run Multi-Agent Users", link: "/how-to/sandbox/run-multi-agent-users" },
                  { text: "Pause Idle Sandboxes", link: "/how-to/sandbox/pause-idle-sandboxes" },
                  { text: "Publish a Snapshot", link: "/how-to/sandbox/publish-a-snapshot" },
                  { text: "Restore from a Snapshot", link: "/how-to/sandbox/restore-from-snapshot" },
                  { text: "Snapshot a Running Sandbox", link: "/how-to/sandbox/snapshot-a-running-sandbox" },
                  { text: "Roll Back a Sandbox", link: "/how-to/sandbox/roll-back-a-sandbox" },
                  { text: "Back Up the Snapshot Registry", link: "/how-to/sandbox/snapshot-backup" },
                ],
              },
              {
                text: "Explanation",
                items: [
                  { text: "Isolation Tiers and Runtimes", link: "/architecture/sandbox/isolation-runtimes" },
                  { text: "Subprocess Isolation", link: "/architecture/sandbox/subprocess-isolation" },
                  { text: "Capability Profiles", link: "/architecture/sandbox/capability-profiles" },
                  { text: "Exec Semantics", link: "/architecture/sandbox/exec-semantics" },
                  { text: "Egress DNS Binding", link: "/architecture/sandbox/egress-dns-binding" },
                  { text: "Volumes", link: "/architecture/sandbox/volumes" },
                  { text: "Snapshot Store", link: "/architecture/sandbox/snapshot-store" },
                  { text: "Snapshot Catalog", link: "/architecture/sandbox/snapshot-catalog" },
                  { text: "Registry RBAC", link: "/architecture/sandbox/registry-rbac" },
                  { text: "KV Key Layout", link: "/architecture/sandbox/kv-key-layout" },
                  { text: "Persistent Sandbox Entity", link: "/architecture/sandbox/entity-rollout" },
                  { text: "Data Retention", link: "/architecture/sandbox/data-retention" },
                  { text: "Agent Substrate Provider", link: "/architecture/sandbox/substrate-provider" },
                ],
              },
              {
                text: "Reference",
                items: [
                  { text: "Documentation Map", link: "/reference/sandbox/documentation-map" },
                  { text: "Sandbox Providers", link: "/reference/sandbox-providers" },
                  { text: "Provider Option Matrix", link: "/reference/sandbox/provider-option-matrix" },
                  { text: "OCI Media Types", link: "/reference/sandbox/media-types" },
                  { text: "Client NATS Permissions", link: "/reference/sandbox/client-nats-permissions" },
                  { text: "Credential Rule Scope", link: "/reference/sandbox/credential-rule-scope" },
                  { text: "Cleanup Receipt Retention", link: "/reference/sandbox/cleanup-receipt-retention" },
                  { text: "Alert Rules", link: "/reference/sandbox/alerts" },
                  { text: "E2B Runtime Contract", link: "/reference/sandbox/e2b-runtime" },
                  { text: "Agent Substrate v1 Contract", link: "/reference/sandbox/substrate-v1-contract" },
                  { text: "Testing Policy", link: "/reference/sandbox/testing-policy" },
                ],
              },
            ],
          },
          {
            text: "Protocols",
            collapsed: true,
            items: [
              {
                text: "Explanation",
                items: [{ text: "A2A, ACP & MCP", link: "/architecture/agent-protocols" }],
              },
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
                items: [
                  { text: "Protocol Support", link: "/reference/agent-protocols" },
                  { text: "A2A", link: "/reference/agent-protocols#a2a" },
                  { text: "ACP", link: "/reference/agent-protocols#acp" },
                  { text: "MCP", link: "/reference/agent-protocols#mcp" },
                  { text: "ACP Conformance", link: "/architecture/acp-conformance" },
                ],
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
                items: [
                  { text: "Container Images", link: "/reference/container-images" },
                  { text: "Gateway Webhooks", link: "/reference/gateway-webhooks" },
                ],
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
                  { text: "Operate Workload Registration", link: "/how-to/operate-workload-registration" },
                  { text: "Integrate Workload Identity", link: "/how-to/integrate-workload-identity" },
                ],
              },
              {
                text: "Reference",
                items: [
                  { text: "Key States", link: "/architecture/key-states" },
                  { text: "Key Wrapping", link: "/reference/key-wrapping" },
                  {
                    text: "Supplied Credential Material",
                    link: "/reference/supplied-credential-material",
                  },
                  { text: "Connection HTTP Proxy", link: "/reference/connection-http-proxy" },
                  { text: "Connector Resource Parents", link: "/reference/connector-resource-parents" },
                  { text: "Connector Security Profile", link: "/reference/connector-security-profile" },
                  { text: "Workload Identity", link: "/reference/workload-identity" },
                  { text: "Workload Identity Configuration", link: "/reference/workload-identity-configuration" },
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
                  { text: "Workload Identity & Trust", link: "/architecture/workload-identity" },
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
              {
                text: "Research Proposals",
                items: [
                  { text: "Portable Workload Identity", link: "/research/workload-identity/README" },
                  { text: "Workload Federation", link: "/research/workload-identity/federation" },
                  { text: "Federation Inventory", link: "/research/workload-identity/federation-inventory" },
                  { text: "JWT-SVID Issuer Spike", link: "/research/workload-identity/jwt-svid-issuer-spike" },
                  { text: "Identity & KMS Adapter Roadmap", link: "/research/workload-identity/adapter-roadmap" },
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
