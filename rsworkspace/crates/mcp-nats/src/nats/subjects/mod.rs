pub mod client;
pub mod markers;
pub mod server;
pub mod subscriptions;

pub mod mcp_server {
    pub use super::server::{
        CallToolSubject, CancelTaskSubject, CancelledSubject, CompleteSubject, ElicitationCompletedSubject,
        GetPromptSubject, GetTaskResultSubject, GetTaskSubject, InitializeSubject, ListPromptsSubject,
        ListResourceTemplatesSubject, ListResourcesSubject, ListTasksSubject, ListToolsSubject, LoggingMessageSubject,
        PingSubject, ProgressSubject, PromptListChangedSubject, ReadResourceSubject, ResourceListChangedSubject,
        ResourceUpdatedSubject, SetLoggingLevelSubject, SubscribeResourceSubject, ToolListChangedSubject,
        UnsubscribeResourceSubject,
    };

    pub mod wildcards {
        pub use super::super::subscriptions::{AllServerSubject, OneServerSubject};
    }
}

pub mod mcp_client {
    pub use super::client::{
        CancelledSubject, CreateElicitationSubject, CreateMessageSubject, InitializedSubject, ListRootsSubject,
        PingSubject, ProgressSubject, RootsListChangedSubject,
    };

    pub mod wildcards {
        pub use super::super::subscriptions::{AllClientSubject, OneClientSubject};
    }
}

#[cfg(test)]
mod tests {
    use super::{mcp_client, mcp_server};
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
            mcp_server::InitializeSubject::new(&p("mcp"), &peer("filesystem")).to_string(),
            "mcp.server.filesystem.initialize"
        );
    }

    #[test]
    fn server_tool_request_subjects_match_mcp_method_groups() {
        assert_eq!(
            mcp_server::ListToolsSubject::new(&p("mcp"), &peer("filesystem")).to_string(),
            "mcp.server.filesystem.tools.list"
        );
        assert_eq!(
            mcp_server::CallToolSubject::new(&p("mcp"), &peer("filesystem")).to_string(),
            "mcp.server.filesystem.tools.call"
        );
    }

    #[test]
    fn all_server_request_subjects_are_method_shaped() {
        let prefix = p("mcp");
        let server = peer("filesystem");
        let subjects = [
            mcp_server::InitializeSubject::new(&prefix, &server).to_string(),
            mcp_server::PingSubject::new(&prefix, &server).to_string(),
            mcp_server::CompleteSubject::new(&prefix, &server).to_string(),
            mcp_server::SetLoggingLevelSubject::new(&prefix, &server).to_string(),
            mcp_server::ListPromptsSubject::new(&prefix, &server).to_string(),
            mcp_server::GetPromptSubject::new(&prefix, &server).to_string(),
            mcp_server::ListResourcesSubject::new(&prefix, &server).to_string(),
            mcp_server::ListResourceTemplatesSubject::new(&prefix, &server).to_string(),
            mcp_server::ReadResourceSubject::new(&prefix, &server).to_string(),
            mcp_server::SubscribeResourceSubject::new(&prefix, &server).to_string(),
            mcp_server::UnsubscribeResourceSubject::new(&prefix, &server).to_string(),
            mcp_server::ListToolsSubject::new(&prefix, &server).to_string(),
            mcp_server::CallToolSubject::new(&prefix, &server).to_string(),
            mcp_server::GetTaskSubject::new(&prefix, &server).to_string(),
            mcp_server::ListTasksSubject::new(&prefix, &server).to_string(),
            mcp_server::GetTaskResultSubject::new(&prefix, &server).to_string(),
            mcp_server::CancelTaskSubject::new(&prefix, &server).to_string(),
        ];

        assert_eq!(
            subjects,
            [
                "mcp.server.filesystem.initialize",
                "mcp.server.filesystem.ping",
                "mcp.server.filesystem.completion.complete",
                "mcp.server.filesystem.logging.set_level",
                "mcp.server.filesystem.prompts.list",
                "mcp.server.filesystem.prompts.get",
                "mcp.server.filesystem.resources.list",
                "mcp.server.filesystem.resources.templates.list",
                "mcp.server.filesystem.resources.read",
                "mcp.server.filesystem.resources.subscribe",
                "mcp.server.filesystem.resources.unsubscribe",
                "mcp.server.filesystem.tools.list",
                "mcp.server.filesystem.tools.call",
                "mcp.server.filesystem.tasks.get",
                "mcp.server.filesystem.tasks.list",
                "mcp.server.filesystem.tasks.result",
                "mcp.server.filesystem.tasks.cancel",
            ]
        );
    }

    #[test]
    fn server_notifications_target_client_namespace() {
        assert_eq!(
            mcp_server::ToolListChangedSubject::new(&p("mcp"), &peer("desktop")).to_string(),
            "mcp.client.desktop.notifications.tools.list_changed"
        );
    }

    #[test]
    fn all_server_notification_subjects_are_peer_targeted() {
        let prefix = p("mcp");
        let client = peer("desktop");
        let subjects = [
            mcp_server::CancelledSubject::new(&prefix, &client).to_string(),
            mcp_server::ProgressSubject::new(&prefix, &client).to_string(),
            mcp_server::LoggingMessageSubject::new(&prefix, &client).to_string(),
            mcp_server::ResourceUpdatedSubject::new(&prefix, &client).to_string(),
            mcp_server::ResourceListChangedSubject::new(&prefix, &client).to_string(),
            mcp_server::ToolListChangedSubject::new(&prefix, &client).to_string(),
            mcp_server::PromptListChangedSubject::new(&prefix, &client).to_string(),
            mcp_server::ElicitationCompletedSubject::new(&prefix, &client).to_string(),
        ];

        assert_eq!(
            subjects,
            [
                "mcp.client.desktop.notifications.cancelled",
                "mcp.client.desktop.notifications.progress",
                "mcp.client.desktop.notifications.message",
                "mcp.client.desktop.notifications.resources.updated",
                "mcp.client.desktop.notifications.resources.list_changed",
                "mcp.client.desktop.notifications.tools.list_changed",
                "mcp.client.desktop.notifications.prompts.list_changed",
                "mcp.client.desktop.notifications.elicitation.complete",
            ]
        );
    }

    #[test]
    fn client_request_subjects_match_mcp_method_groups() {
        assert_eq!(
            mcp_client::CreateMessageSubject::new(&p("mcp"), &peer("desktop")).to_string(),
            "mcp.client.desktop.sampling.create_message"
        );
        assert_eq!(
            mcp_client::ListRootsSubject::new(&p("mcp"), &peer("desktop")).to_string(),
            "mcp.client.desktop.roots.list"
        );
    }

    #[test]
    fn all_client_subjects_are_peer_targeted() {
        let prefix = p("mcp");
        let client = peer("desktop");
        let server = peer("filesystem");
        let subjects = [
            mcp_client::PingSubject::new(&prefix, &client).to_string(),
            mcp_client::CreateMessageSubject::new(&prefix, &client).to_string(),
            mcp_client::ListRootsSubject::new(&prefix, &client).to_string(),
            mcp_client::CreateElicitationSubject::new(&prefix, &client).to_string(),
            mcp_client::CancelledSubject::new(&prefix, &server).to_string(),
            mcp_client::ProgressSubject::new(&prefix, &server).to_string(),
            mcp_client::InitializedSubject::new(&prefix, &server).to_string(),
            mcp_client::RootsListChangedSubject::new(&prefix, &server).to_string(),
        ];

        assert_eq!(
            subjects,
            [
                "mcp.client.desktop.ping",
                "mcp.client.desktop.sampling.create_message",
                "mcp.client.desktop.roots.list",
                "mcp.client.desktop.elicitation.create",
                "mcp.server.filesystem.notifications.cancelled",
                "mcp.server.filesystem.notifications.progress",
                "mcp.server.filesystem.notifications.initialized",
                "mcp.server.filesystem.notifications.roots.list_changed",
            ]
        );
    }

    #[test]
    fn wildcards_match_acp_export_pattern() {
        assert_eq!(
            mcp_server::wildcards::AllServerSubject::new(&p("mcp")).to_string(),
            "mcp.server.>"
        );
        assert_eq!(
            mcp_client::wildcards::AllClientSubject::new(&p("mcp")).to_string(),
            "mcp.client.>"
        );
        assert_eq!(
            mcp_client::wildcards::OneClientSubject::new(&p("mcp"), &peer("desktop")).to_string(),
            "mcp.client.desktop.>"
        );
    }
}
