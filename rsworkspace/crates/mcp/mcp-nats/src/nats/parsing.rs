use crate::McpPeerId;

macro_rules! suffix_method {
    ($name:ident { $($variant:ident => $suffix:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            fn from_suffix(suffix: &str) -> Option<Self> {
                match suffix {
                    $($suffix => Some(Self::$variant),)+
                    _ => None,
                }
            }

            #[cfg(test)]
            pub(crate) const SUFFIXES: &[&str] = &[$($suffix),+];
        }
    };
}

suffix_method! {
    ServerRequestMethod {
        Initialize => "initialize",
        Ping => "ping",
        Discover => "server.discover",
        Complete => "completion.complete",
        SetLoggingLevel => "logging.set_level",
        ListPrompts => "prompts.list",
        GetPrompt => "prompts.get",
        ListResources => "resources.list",
        ListResourceTemplates => "resources.templates.list",
        ReadResource => "resources.read",
        SubscriptionsListen => "subscriptions.listen",
        SubscribeResource => "resources.subscribe",
        UnsubscribeResource => "resources.unsubscribe",
        ListTools => "tools.list",
        CallTool => "tools.call",
        GetTask => "tasks.get",
        UpdateTask => "tasks.update",
        CancelTask => "tasks.cancel",
    }
}

suffix_method! {
    ServerNotificationMethod {
        Cancelled => "notifications.cancelled",
        Progress => "notifications.progress",
        LoggingMessage => "notifications.message",
        ResourceUpdated => "notifications.resources.updated",
        ResourceListChanged => "notifications.resources.list_changed",
        ToolListChanged => "notifications.tools.list_changed",
        PromptListChanged => "notifications.prompts.list_changed",
        TaskStatus => "notifications.tasks",
        SubscriptionsAcknowledged => "notifications.subscriptions.acknowledged",
    }
}

suffix_method! {
    ClientRequestMethod {
        Ping => "ping",
        CreateMessage => "sampling.create_message",
        ListRoots => "roots.list",
        CreateElicitation => "elicitation.create",
    }
}

suffix_method! {
    ClientNotificationMethod {
        Cancelled => "notifications.cancelled",
        Progress => "notifications.progress",
        Initialized => "notifications.initialized",
        RootsListChanged => "notifications.roots.list_changed",
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
mod tests;
