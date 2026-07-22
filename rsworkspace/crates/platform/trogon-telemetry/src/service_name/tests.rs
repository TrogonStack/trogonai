use super::*;

#[test]
fn as_str_returns_expected_values() {
    assert_eq!(ServiceName::AcpNatsStdio.as_str(), "acp-nats-stdio");
    assert_eq!(ServiceName::AcpNatsServer.as_str(), "acp-nats-server");
    assert_eq!(ServiceName::McpNatsStdio.as_str(), "mcp-nats-stdio");
    assert_eq!(ServiceName::McpNatsServer.as_str(), "mcp-nats-server");
    assert_eq!(ServiceName::TrogonGateway.as_str(), "trogon-gateway");
    assert_eq!(ServiceName::TrogonSourceDiscord.as_str(), "trogon-source-discord");
    assert_eq!(ServiceName::TrogonSourceGithub.as_str(), "trogon-source-github");
    assert_eq!(ServiceName::TrogonSourceGitlab.as_str(), "trogon-source-gitlab");
    assert_eq!(ServiceName::TrogonSourceLinear.as_str(), "trogon-source-linear");
    assert_eq!(ServiceName::TrogonSourceSlack.as_str(), "trogon-source-slack");
    assert_eq!(ServiceName::TrogonSourceTelegram.as_str(), "trogon-source-telegram");
}

#[test]
fn display_delegates_to_as_str() {
    assert_eq!(format!("{}", ServiceName::AcpNatsStdio), "acp-nats-stdio");
    assert_eq!(format!("{}", ServiceName::AcpNatsServer), "acp-nats-server");
    assert_eq!(format!("{}", ServiceName::McpNatsStdio), "mcp-nats-stdio");
    assert_eq!(format!("{}", ServiceName::McpNatsServer), "mcp-nats-server");
    assert_eq!(format!("{}", ServiceName::TrogonGateway), "trogon-gateway");
    assert_eq!(format!("{}", ServiceName::TrogonSourceDiscord), "trogon-source-discord");
    assert_eq!(format!("{}", ServiceName::TrogonSourceGithub), "trogon-source-github");
    assert_eq!(format!("{}", ServiceName::TrogonSourceGitlab), "trogon-source-gitlab");
    assert_eq!(format!("{}", ServiceName::TrogonSourceLinear), "trogon-source-linear");
    assert_eq!(format!("{}", ServiceName::TrogonSourceSlack), "trogon-source-slack");
    assert_eq!(
        format!("{}", ServiceName::TrogonSourceTelegram),
        "trogon-source-telegram"
    );
}
