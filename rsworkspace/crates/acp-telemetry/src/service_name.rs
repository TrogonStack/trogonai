/// Known ACP service binaries. Used as the OTel service name, log directory
/// name, and log file name — keeping them in one place prevents typos and
/// guarantees the values are path-safe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceName {
    AcpNatsStdio,
    AcpNatsWs,
    TrogonGateway,
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
            Self::AcpNatsWs => "acp-nats-ws",
            Self::TrogonGateway => "trogon-gateway",
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
mod tests {
    use super::*;

    #[test]
    fn as_str_returns_expected_values() {
        assert_eq!(ServiceName::AcpNatsStdio.as_str(), "acp-nats-stdio");
        assert_eq!(ServiceName::AcpNatsWs.as_str(), "acp-nats-ws");
        assert_eq!(ServiceName::TrogonGateway.as_str(), "trogon-gateway");
        assert_eq!(
            ServiceName::TrogonSourceDiscord.as_str(),
            "trogon-source-discord"
        );
        assert_eq!(
            ServiceName::TrogonSourceGithub.as_str(),
            "trogon-source-github"
        );
        assert_eq!(
            ServiceName::TrogonSourceGitlab.as_str(),
            "trogon-source-gitlab"
        );
        assert_eq!(
            ServiceName::TrogonSourceLinear.as_str(),
            "trogon-source-linear"
        );
        assert_eq!(
            ServiceName::TrogonSourceSlack.as_str(),
            "trogon-source-slack"
        );
        assert_eq!(
            ServiceName::TrogonSourceTelegram.as_str(),
            "trogon-source-telegram"
        );
    }

    #[test]
    fn display_delegates_to_as_str() {
        assert_eq!(format!("{}", ServiceName::AcpNatsStdio), "acp-nats-stdio");
        assert_eq!(format!("{}", ServiceName::AcpNatsWs), "acp-nats-ws");
        assert_eq!(format!("{}", ServiceName::TrogonGateway), "trogon-gateway");
        assert_eq!(
            format!("{}", ServiceName::TrogonSourceDiscord),
            "trogon-source-discord"
        );
        assert_eq!(
            format!("{}", ServiceName::TrogonSourceGithub),
            "trogon-source-github"
        );
        assert_eq!(
            format!("{}", ServiceName::TrogonSourceGitlab),
            "trogon-source-gitlab"
        );
        assert_eq!(
            format!("{}", ServiceName::TrogonSourceLinear),
            "trogon-source-linear"
        );
        assert_eq!(
            format!("{}", ServiceName::TrogonSourceSlack),
            "trogon-source-slack"
        );
        assert_eq!(
            format!("{}", ServiceName::TrogonSourceTelegram),
            "trogon-source-telegram"
        );
    }
}
