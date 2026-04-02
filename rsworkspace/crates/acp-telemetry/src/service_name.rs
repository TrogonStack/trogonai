/// Known ACP service binaries. Used as the OTel service name, log directory
/// name, and log file name — keeping them in one place prevents typos and
/// guarantees the values are path-safe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceName {
    AcpNatsStdio,
    AcpNatsWs,
    TrogonSinkGithub,
}

impl ServiceName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AcpNatsStdio => "acp-nats-stdio",
            Self::AcpNatsWs => "acp-nats-ws",
            Self::TrogonSinkGithub => "trogon-sink-github",
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
        assert_eq!(ServiceName::TrogonSinkGithub.as_str(), "trogon-sink-github");
    }

    #[test]
    fn display_delegates_to_as_str() {
        assert_eq!(format!("{}", ServiceName::AcpNatsStdio), "acp-nats-stdio");
        assert_eq!(format!("{}", ServiceName::AcpNatsWs), "acp-nats-ws");
        assert_eq!(
            format!("{}", ServiceName::TrogonSinkGithub),
            "trogon-sink-github"
        );
    }
}
