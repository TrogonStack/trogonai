use super::{client, server, subscriptions};
use crate::{McpPeerId, McpPrefix};

fn p(s: &str) -> McpPrefix {
    McpPrefix::new(s).unwrap()
}

fn peer(s: &str) -> McpPeerId {
    McpPeerId::new(s).unwrap()
}

#[test]
fn server_initialize_subject_matches_acp_style() {
    assert_eq!(
        server::InitializeSubject::new(&p("mcp"), &peer("filesystem")).to_string(),
        "mcp.v1.server.filesystem.initialize"
    );
}

#[test]
fn server_tool_request_subjects_match_mcp_method_groups() {
    assert_eq!(
        server::ListToolsSubject::new(&p("mcp"), &peer("filesystem")).to_string(),
        "mcp.v1.server.filesystem.tools.list"
    );
    assert_eq!(
        server::CallToolSubject::new(&p("mcp"), &peer("filesystem")).to_string(),
        "mcp.v1.server.filesystem.tools.call"
    );
}

#[test]
fn all_server_request_subjects_are_method_shaped() {
    let prefix = p("mcp");
    let server = peer("filesystem");
    let subjects = [
        server::InitializeSubject::new(&prefix, &server).to_string(),
        server::PingSubject::new(&prefix, &server).to_string(),
        server::DiscoverSubject::new(&prefix, &server).to_string(),
        server::CompleteSubject::new(&prefix, &server).to_string(),
        server::SetLoggingLevelSubject::new(&prefix, &server).to_string(),
        server::ListPromptsSubject::new(&prefix, &server).to_string(),
        server::GetPromptSubject::new(&prefix, &server).to_string(),
        server::ListResourcesSubject::new(&prefix, &server).to_string(),
        server::ListResourceTemplatesSubject::new(&prefix, &server).to_string(),
        server::ReadResourceSubject::new(&prefix, &server).to_string(),
        server::SubscriptionsListenSubject::new(&prefix, &server).to_string(),
        server::SubscribeResourceSubject::new(&prefix, &server).to_string(),
        server::UnsubscribeResourceSubject::new(&prefix, &server).to_string(),
        server::ListToolsSubject::new(&prefix, &server).to_string(),
        server::CallToolSubject::new(&prefix, &server).to_string(),
        server::GetTaskSubject::new(&prefix, &server).to_string(),
        server::UpdateTaskSubject::new(&prefix, &server).to_string(),
        server::CancelTaskSubject::new(&prefix, &server).to_string(),
    ];

    assert_eq!(
        subjects,
        [
            "mcp.v1.server.filesystem.initialize",
            "mcp.v1.server.filesystem.ping",
            "mcp.v1.server.filesystem.server.discover",
            "mcp.v1.server.filesystem.completion.complete",
            "mcp.v1.server.filesystem.logging.set_level",
            "mcp.v1.server.filesystem.prompts.list",
            "mcp.v1.server.filesystem.prompts.get",
            "mcp.v1.server.filesystem.resources.list",
            "mcp.v1.server.filesystem.resources.templates.list",
            "mcp.v1.server.filesystem.resources.read",
            "mcp.v1.server.filesystem.subscriptions.listen",
            "mcp.v1.server.filesystem.resources.subscribe",
            "mcp.v1.server.filesystem.resources.unsubscribe",
            "mcp.v1.server.filesystem.tools.list",
            "mcp.v1.server.filesystem.tools.call",
            "mcp.v1.server.filesystem.tasks.get",
            "mcp.v1.server.filesystem.tasks.update",
            "mcp.v1.server.filesystem.tasks.cancel",
        ]
    );
}

#[test]
fn server_notifications_target_client_namespace() {
    assert_eq!(
        server::ToolListChangedSubject::new(&p("mcp"), &peer("desktop")).to_string(),
        "mcp.v1.client.desktop.notifications.tools.list_changed"
    );
}

#[test]
fn all_server_notification_subjects_are_peer_targeted() {
    let prefix = p("mcp");
    let client = peer("desktop");
    let subjects = [
        server::CancelledSubject::new(&prefix, &client).to_string(),
        server::ProgressSubject::new(&prefix, &client).to_string(),
        server::LoggingMessageSubject::new(&prefix, &client).to_string(),
        server::ResourceUpdatedSubject::new(&prefix, &client).to_string(),
        server::ResourceListChangedSubject::new(&prefix, &client).to_string(),
        server::ToolListChangedSubject::new(&prefix, &client).to_string(),
        server::PromptListChangedSubject::new(&prefix, &client).to_string(),
        server::SubscriptionsAcknowledgedSubject::new(&prefix, &client).to_string(),
        server::TaskStatusSubject::new(&prefix, &client).to_string(),
    ];

    assert_eq!(
        subjects,
        [
            "mcp.v1.client.desktop.notifications.cancelled",
            "mcp.v1.client.desktop.notifications.progress",
            "mcp.v1.client.desktop.notifications.message",
            "mcp.v1.client.desktop.notifications.resources.updated",
            "mcp.v1.client.desktop.notifications.resources.list_changed",
            "mcp.v1.client.desktop.notifications.tools.list_changed",
            "mcp.v1.client.desktop.notifications.prompts.list_changed",
            "mcp.v1.client.desktop.notifications.subscriptions.acknowledged",
            "mcp.v1.client.desktop.notifications.tasks",
        ]
    );
}

#[test]
fn client_request_subjects_match_mcp_method_groups() {
    assert_eq!(
        client::CreateMessageSubject::new(&p("mcp"), &peer("desktop")).to_string(),
        "mcp.v1.client.desktop.sampling.create_message"
    );
    assert_eq!(
        client::ListRootsSubject::new(&p("mcp"), &peer("desktop")).to_string(),
        "mcp.v1.client.desktop.roots.list"
    );
}

#[test]
fn all_client_subjects_are_peer_targeted() {
    let prefix = p("mcp");
    let client = peer("desktop");
    let server = peer("filesystem");
    let subjects = [
        client::PingSubject::new(&prefix, &client).to_string(),
        client::CreateMessageSubject::new(&prefix, &client).to_string(),
        client::ListRootsSubject::new(&prefix, &client).to_string(),
        client::CreateElicitationSubject::new(&prefix, &client).to_string(),
        client::CancelledSubject::new(&prefix, &server).to_string(),
        client::ProgressSubject::new(&prefix, &server).to_string(),
        client::InitializedSubject::new(&prefix, &server).to_string(),
        client::RootsListChangedSubject::new(&prefix, &server).to_string(),
    ];

    assert_eq!(
        subjects,
        [
            "mcp.v1.client.desktop.ping",
            "mcp.v1.client.desktop.sampling.create_message",
            "mcp.v1.client.desktop.roots.list",
            "mcp.v1.client.desktop.elicitation.create",
            "mcp.v1.server.filesystem.notifications.cancelled",
            "mcp.v1.server.filesystem.notifications.progress",
            "mcp.v1.server.filesystem.notifications.initialized",
            "mcp.v1.server.filesystem.notifications.roots.list_changed",
        ]
    );
}

#[test]
fn wildcards_match_acp_export_pattern() {
    assert_eq!(
        subscriptions::AllServerSubject::new(&p("mcp")).to_string(),
        "mcp.v1.server.>"
    );
    assert_eq!(
        subscriptions::AllClientSubject::new(&p("mcp")).to_string(),
        "mcp.v1.client.>"
    );
    assert_eq!(
        subscriptions::OneClientSubject::new(&p("mcp"), &peer("desktop")).to_string(),
        "mcp.v1.client.desktop.>"
    );
}
