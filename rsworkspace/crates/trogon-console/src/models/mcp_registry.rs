use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct McpServer {
    pub name: &'static str,
    pub url: &'static str,
}

/// Static list of well-known MCP servers.
pub fn known_mcp_servers() -> Vec<McpServer> {
    vec![
        McpServer {
            name: "Airtable",
            url: "https://mcp.airtable.com/mcp",
        },
        McpServer {
            name: "Amplitude",
            url: "https://mcp.amplitude.com/mcp",
        },
        McpServer {
            name: "Apollo.io",
            url: "https://mcp.apollo.io/mcp",
        },
        McpServer {
            name: "Asana",
            url: "https://mcp.asana.com/v2/mcp",
        },
        McpServer {
            name: "Atlassian Rovo",
            url: "https://mcp.atlassian.com/v1/mcp",
        },
        McpServer {
            name: "Attio",
            url: "https://mcp.attio.com/mcp",
        },
        McpServer {
            name: "GitHub",
            url: "https://mcp.github.com/mcp",
        },
        McpServer {
            name: "Linear",
            url: "https://mcp.linear.app/mcp",
        },
        McpServer {
            name: "Notion",
            url: "https://mcp.notion.com/mcp",
        },
        McpServer {
            name: "Slack",
            url: "https://mcp.slack.com/mcp",
        },
    ]
}
