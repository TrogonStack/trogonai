/// Known Trogon service binaries. Used as the OTel service name, log directory
/// name, and log file name — keeping them in one place prevents typos and
/// guarantees the values are path-safe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceName {
    AcpNatsStdio,
    AcpNatsServer,
    AcpNatsWs,
    McpNatsStdio,
    McpNatsServer,
    TrogonCli,
    TrogonAcpRunner,
    TrogonCompactor,
    TrogonGateway,
    TrogonPrActor,
    TrogonRouter,
    TrogonSourceDiscord,
    TrogonSourceGithub,
    TrogonSourceGitlab,
    TrogonSourceLinear,
    TrogonSourceSlack,
    TrogonSourceTelegram,
}

impl ServiceName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AcpNatsStdio => "acp-nats-stdio",
            Self::AcpNatsServer => "acp-nats-server",
            Self::AcpNatsWs => "acp-nats-ws",
            Self::McpNatsStdio => "mcp-nats-stdio",
            Self::McpNatsServer => "mcp-nats-server",
            Self::TrogonCli => "trogon-cli",
            Self::TrogonAcpRunner => "trogon-acp-runner",
            Self::TrogonCompactor => "trogon-compactor",
            Self::TrogonGateway => "trogon-gateway",
            Self::TrogonPrActor => "trogon-pr-actor",
            Self::TrogonRouter => "trogon-router",
            Self::TrogonSourceDiscord => "trogon-source-discord",
            Self::TrogonSourceGithub => "trogon-source-github",
            Self::TrogonSourceGitlab => "trogon-source-gitlab",
            Self::TrogonSourceLinear => "trogon-source-linear",
            Self::TrogonSourceSlack => "trogon-source-slack",
            Self::TrogonSourceTelegram => "trogon-source-telegram",
        }
    }
}

impl std::fmt::Display for ServiceName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
