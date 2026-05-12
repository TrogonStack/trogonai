use crate::McpPeerId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerRequestMethod {
    Initialize,
    Ping,
    Complete,
    SetLoggingLevel,
    ListPrompts,
    GetPrompt,
    ListResources,
    ListResourceTemplates,
    ReadResource,
    SubscribeResource,
    UnsubscribeResource,
    ListTools,
    CallTool,
    GetTask,
    ListTasks,
    GetTaskResult,
    CancelTask,
}

impl ServerRequestMethod {
    fn from_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "initialize" => Some(Self::Initialize),
            "ping" => Some(Self::Ping),
            "completion.complete" => Some(Self::Complete),
            "logging.set_level" => Some(Self::SetLoggingLevel),
            "prompts.list" => Some(Self::ListPrompts),
            "prompts.get" => Some(Self::GetPrompt),
            "resources.list" => Some(Self::ListResources),
            "resources.templates.list" => Some(Self::ListResourceTemplates),
            "resources.read" => Some(Self::ReadResource),
            "resources.subscribe" => Some(Self::SubscribeResource),
            "resources.unsubscribe" => Some(Self::UnsubscribeResource),
            "tools.list" => Some(Self::ListTools),
            "tools.call" => Some(Self::CallTool),
            "tasks.get" => Some(Self::GetTask),
            "tasks.list" => Some(Self::ListTasks),
            "tasks.result" => Some(Self::GetTaskResult),
            "tasks.cancel" => Some(Self::CancelTask),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerNotificationMethod {
    Cancelled,
    Progress,
    LoggingMessage,
    ResourceUpdated,
    ResourceListChanged,
    ToolListChanged,
    PromptListChanged,
    ElicitationCompleted,
}

impl ServerNotificationMethod {
    fn from_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "notifications.cancelled" => Some(Self::Cancelled),
            "notifications.progress" => Some(Self::Progress),
            "notifications.message" => Some(Self::LoggingMessage),
            "notifications.resources.updated" => Some(Self::ResourceUpdated),
            "notifications.resources.list_changed" => Some(Self::ResourceListChanged),
            "notifications.tools.list_changed" => Some(Self::ToolListChanged),
            "notifications.prompts.list_changed" => Some(Self::PromptListChanged),
            "notifications.elicitation.complete" => Some(Self::ElicitationCompleted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientRequestMethod {
    Ping,
    CreateMessage,
    ListRoots,
    CreateElicitation,
}

impl ClientRequestMethod {
    fn from_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "ping" => Some(Self::Ping),
            "sampling.create_message" => Some(Self::CreateMessage),
            "roots.list" => Some(Self::ListRoots),
            "elicitation.create" => Some(Self::CreateElicitation),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientNotificationMethod {
    Cancelled,
    Progress,
    Initialized,
    RootsListChanged,
}

impl ClientNotificationMethod {
    fn from_suffix(suffix: &str) -> Option<Self> {
        match suffix {
            "notifications.cancelled" => Some(Self::Cancelled),
            "notifications.progress" => Some(Self::Progress),
            "notifications.initialized" => Some(Self::Initialized),
            "notifications.roots.list_changed" => Some(Self::RootsListChanged),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedServerSubject {
    Request {
        server_id: McpPeerId,
        method: ServerRequestMethod,
    },
    Notification {
        server_id: McpPeerId,
        method: ClientNotificationMethod,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedClientSubject {
    Request {
        client_id: McpPeerId,
        method: ClientRequestMethod,
    },
    Notification {
        client_id: McpPeerId,
        method: ServerNotificationMethod,
    },
}

pub fn parse_server_subject(subject: &str) -> Option<ParsedServerSubject> {
    if let Some((peer_id, method)) = parse_role_subject(subject, ".server.", ServerRequestMethod::from_suffix) {
        return Some(ParsedServerSubject::Request {
            server_id: peer_id,
            method,
        });
    }
    if let Some((peer_id, method)) = parse_role_subject(subject, ".server.", ClientNotificationMethod::from_suffix) {
        return Some(ParsedServerSubject::Notification {
            server_id: peer_id,
            method,
        });
    }
    None
}

pub fn parse_client_subject(subject: &str) -> Option<ParsedClientSubject> {
    if let Some((peer_id, method)) = parse_role_subject(subject, ".client.", ClientRequestMethod::from_suffix) {
        return Some(ParsedClientSubject::Request {
            client_id: peer_id,
            method,
        });
    }
    if let Some((peer_id, method)) = parse_role_subject(subject, ".client.", ServerNotificationMethod::from_suffix) {
        return Some(ParsedClientSubject::Notification {
            client_id: peer_id,
            method,
        });
    }
    None
}

fn parse_role_subject<T>(
    subject: &str,
    marker: &str,
    parse_method: impl Fn(&str) -> Option<T>,
) -> Option<(McpPeerId, T)> {
    let mut search_start = 0;
    while let Some(offset) = subject[search_start..].find(marker) {
        let role_pos = search_start + offset;
        search_start = role_pos + 1;

        let after_role = &subject[role_pos + marker.len()..];
        let Some((peer, suffix)) = after_role.split_once('.') else {
            continue;
        };
        let Ok(peer_id) = McpPeerId::new(peer) else {
            continue;
        };
        let Some(method) = parse_method(suffix) else {
            continue;
        };
        return Some((peer_id, method));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(s: &str) -> McpPeerId {
        McpPeerId::new(s).unwrap()
    }

    #[test]
    fn parses_server_request_subject() {
        assert_eq!(
            parse_server_subject("mcp.server.filesystem.tools.list").unwrap(),
            ParsedServerSubject::Request {
                server_id: peer("filesystem"),
                method: ServerRequestMethod::ListTools,
            }
        );
    }

    #[test]
    fn parses_server_subject_when_peer_id_matches_role_name() {
        assert_eq!(
            parse_server_subject("mcp.server.server.tools.list").unwrap(),
            ParsedServerSubject::Request {
                server_id: peer("server"),
                method: ServerRequestMethod::ListTools,
            }
        );
        assert_eq!(
            parse_server_subject("mcp.server.server.notifications.initialized").unwrap(),
            ParsedServerSubject::Notification {
                server_id: peer("server"),
                method: ClientNotificationMethod::Initialized,
            }
        );
    }

    #[test]
    fn parses_server_subject_when_prefix_contains_role_name() {
        assert_eq!(
            parse_server_subject("mcp.server.namespace.server.server.tools.list").unwrap(),
            ParsedServerSubject::Request {
                server_id: peer("server"),
                method: ServerRequestMethod::ListTools,
            }
        );
    }

    #[test]
    fn parses_server_notification_subject() {
        assert_eq!(
            parse_server_subject("mcp.server.filesystem.notifications.initialized").unwrap(),
            ParsedServerSubject::Notification {
                server_id: peer("filesystem"),
                method: ClientNotificationMethod::Initialized,
            }
        );
    }

    #[test]
    fn parses_client_request_subject() {
        assert_eq!(
            parse_client_subject("mcp.client.desktop.sampling.create_message").unwrap(),
            ParsedClientSubject::Request {
                client_id: peer("desktop"),
                method: ClientRequestMethod::CreateMessage,
            }
        );
    }

    #[test]
    fn parses_client_subject_when_peer_id_matches_role_name() {
        assert_eq!(
            parse_client_subject("mcp.client.client.roots.list").unwrap(),
            ParsedClientSubject::Request {
                client_id: peer("client"),
                method: ClientRequestMethod::ListRoots,
            }
        );
        assert_eq!(
            parse_client_subject("mcp.client.client.notifications.tools.list_changed").unwrap(),
            ParsedClientSubject::Notification {
                client_id: peer("client"),
                method: ServerNotificationMethod::ToolListChanged,
            }
        );
    }

    #[test]
    fn parses_client_subject_when_prefix_contains_role_name() {
        assert_eq!(
            parse_client_subject("mcp.client.namespace.client.client.roots.list").unwrap(),
            ParsedClientSubject::Request {
                client_id: peer("client"),
                method: ClientRequestMethod::ListRoots,
            }
        );
    }

    #[test]
    fn parses_client_notification_subject() {
        assert_eq!(
            parse_client_subject("mcp.client.desktop.notifications.resources.updated").unwrap(),
            ParsedClientSubject::Notification {
                client_id: peer("desktop"),
                method: ServerNotificationMethod::ResourceUpdated,
            }
        );
    }

    #[test]
    fn rejects_unknown_or_invalid_subjects() {
        assert!(parse_server_subject("mcp.server.filesystem").is_none());
        assert_eq!(
            parse_server_subject("mcp.server..server.filesystem.tools.list").unwrap(),
            ParsedServerSubject::Request {
                server_id: peer("filesystem"),
                method: ServerRequestMethod::ListTools,
            }
        );
        assert!(parse_server_subject("mcp.server.filesystem.request").is_none());
        assert!(parse_server_subject("mcp.server.filesystem.notifications.resources.updated").is_none());
        assert!(parse_server_subject("mcp.server.file.system.tools.list").is_none());
        assert!(parse_client_subject("mcp.client.desktop.tools.list").is_none());
        assert!(parse_client_subject("mcp.client.desktop.notifications.initialized").is_none());
    }
}
