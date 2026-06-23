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
