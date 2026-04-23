use async_nats::jetstream::stream::{Config, DiscardPolicy, RetentionPolicy, StorageType};

use crate::acp_prefix::AcpPrefix;
use crate::constants::DEFAULT_STREAM_MAX_AGE;

/// The JetStream stream that captures a subject's messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpStream {
    Commands,
    Responses,
    ClientOps,
    Notifications,
    Global,
    GlobalExt,
}

impl AcpStream {
    pub const ALL: [AcpStream; 6] = [
        Self::Commands,
        Self::Responses,
        Self::ClientOps,
        Self::Notifications,
        Self::Global,
        Self::GlobalExt,
    ];

    pub fn suffix(&self) -> &'static str {
        match self {
            Self::Commands => "COMMANDS",
            Self::Responses => "RESPONSES",
            Self::ClientOps => "CLIENT_OPS",
            Self::Notifications => "NOTIFICATIONS",
            Self::Global => "GLOBAL",
            Self::GlobalExt => "GLOBAL_EXT",
        }
    }

    pub fn stream_name(&self, prefix: &AcpPrefix) -> String {
        format!(
            "{}_{}",
            prefix.as_str().to_uppercase().replace('.', "_"),
            self.suffix()
        )
    }

    pub fn subject_patterns(&self, prefix: &AcpPrefix) -> Vec<String> {
        let p = prefix.as_str();
        match self {
            Self::Commands => vec![
                format!("{p}.session.*.agent.prompt"),
                format!("{p}.session.*.agent.cancel"),
                format!("{p}.session.*.agent.steer"),
                format!("{p}.session.*.agent.load"),
                format!("{p}.session.*.agent.set_mode"),
                format!("{p}.session.*.agent.set_config_option"),
                format!("{p}.session.*.agent.set_model"),
                format!("{p}.session.*.agent.fork"),
                format!("{p}.session.*.agent.resume"),
                format!("{p}.session.*.agent.close"),
            ],
            Self::Responses => vec![
                format!("{p}.session.*.agent.prompt.response.>"),
                format!("{p}.session.*.agent.response.>"),
                format!("{p}.session.*.agent.ext.ready"),
                format!("{p}.session.*.agent.cancelled"),
            ],
            Self::ClientOps => vec![format!("{p}.session.*.client.>")],
            Self::Notifications => vec![format!("{p}.session.*.agent.update.>")],
            Self::Global => vec![
                format!("{p}.agent.initialize"),
                format!("{p}.agent.authenticate"),
                format!("{p}.agent.logout"),
                format!("{p}.agent.session.new"),
            ],
            Self::GlobalExt => vec![format!("{p}.agent.ext.>")],
        }
    }

    pub fn config(&self, prefix: &AcpPrefix) -> Config {
        // Global and GlobalExt subjects are used for core NATS request-reply.
        // no_ack prevents JetStream from sending PubAcks to the request reply-to
        // inbox, which would race with the runner's actual response.
        let no_ack = matches!(self, Self::Global | Self::GlobalExt);
        Config {
            name: self.stream_name(prefix),
            subjects: self.subject_patterns(prefix),
            storage: StorageType::File,
            retention: RetentionPolicy::Limits,
            max_age: DEFAULT_STREAM_MAX_AGE,
            discard: DiscardPolicy::Old,
            no_ack,
            ..Default::default()
        }
    }

    pub fn all_configs(prefix: &AcpPrefix) -> [Config; 6] {
        Self::ALL.map(|s| s.config(prefix))
    }
}

impl std::fmt::Display for AcpStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.suffix())
    }
}

/// A subject knows which stream captures it (if any).
pub trait StreamAssignment {
    const STREAM: Option<AcpStream>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp_prefix::AcpPrefix;

    fn prefix(s: &str) -> AcpPrefix {
        AcpPrefix::new(s).unwrap()
    }

    #[test]
    fn all_array_contains_all_six_variants() {
        assert_eq!(AcpStream::ALL.len(), 6);
        assert!(AcpStream::ALL.contains(&AcpStream::Commands));
        assert!(AcpStream::ALL.contains(&AcpStream::Responses));
        assert!(AcpStream::ALL.contains(&AcpStream::ClientOps));
        assert!(AcpStream::ALL.contains(&AcpStream::Notifications));
        assert!(AcpStream::ALL.contains(&AcpStream::Global));
        assert!(AcpStream::ALL.contains(&AcpStream::GlobalExt));
    }

    #[test]
    fn suffix_returns_correct_static_string_for_each_variant() {
        assert_eq!(AcpStream::Commands.suffix(), "COMMANDS");
        assert_eq!(AcpStream::Responses.suffix(), "RESPONSES");
        assert_eq!(AcpStream::ClientOps.suffix(), "CLIENT_OPS");
        assert_eq!(AcpStream::Notifications.suffix(), "NOTIFICATIONS");
        assert_eq!(AcpStream::Global.suffix(), "GLOBAL");
        assert_eq!(AcpStream::GlobalExt.suffix(), "GLOBAL_EXT");
    }

    #[test]
    fn stream_name_uppercases_prefix_and_appends_suffix() {
        let p = prefix("acp");
        assert_eq!(AcpStream::Commands.stream_name(&p), "ACP_COMMANDS");
        assert_eq!(AcpStream::Global.stream_name(&p), "ACP_GLOBAL");
    }

    #[test]
    fn stream_name_replaces_dots_with_underscores() {
        let p = prefix("my.multi.part");
        assert_eq!(
            AcpStream::Commands.stream_name(&p),
            "MY_MULTI_PART_COMMANDS"
        );
        assert_eq!(
            AcpStream::GlobalExt.stream_name(&p),
            "MY_MULTI_PART_GLOBAL_EXT"
        );
    }

    #[test]
    fn subject_patterns_for_commands_contain_session_agent_paths() {
        let p = prefix("acp");
        let patterns = AcpStream::Commands.subject_patterns(&p);
        assert!(!patterns.is_empty());
        assert!(patterns.contains(&"acp.session.*.agent.prompt".to_string()));
        assert!(patterns.contains(&"acp.session.*.agent.cancel".to_string()));
        assert!(patterns.contains(&"acp.session.*.agent.load".to_string()));
        for pat in &patterns {
            assert!(
                pat.starts_with("acp."),
                "every pattern must start with prefix: {pat}"
            );
        }
    }

    #[test]
    fn subject_patterns_for_responses_contain_response_paths() {
        let p = prefix("acp");
        let patterns = AcpStream::Responses.subject_patterns(&p);
        assert!(patterns.contains(&"acp.session.*.agent.ext.ready".to_string()));
        assert!(patterns.iter().any(|p| p.ends_with('>')));
    }

    #[test]
    fn subject_patterns_for_global_contain_initialize_and_authenticate() {
        let p = prefix("acp");
        let patterns = AcpStream::Global.subject_patterns(&p);
        assert!(patterns.contains(&"acp.agent.initialize".to_string()));
        assert!(patterns.contains(&"acp.agent.authenticate".to_string()));
        assert!(patterns.contains(&"acp.agent.logout".to_string()));
        assert!(patterns.contains(&"acp.agent.session.new".to_string()));
    }

    #[test]
    fn subject_patterns_for_global_ext_is_single_wildcard_pattern() {
        let p = prefix("myns");
        let patterns = AcpStream::GlobalExt.subject_patterns(&p);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0], "myns.agent.ext.>");
    }

    #[test]
    fn subject_patterns_for_client_ops_is_single_wildcard_pattern() {
        let p = prefix("acp");
        let patterns = AcpStream::ClientOps.subject_patterns(&p);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0], "acp.session.*.client.>");
    }

    #[test]
    fn config_name_equals_stream_name() {
        let p = prefix("acp");
        for variant in AcpStream::ALL {
            let cfg = variant.config(&p);
            assert_eq!(
                cfg.name,
                variant.stream_name(&p),
                "config.name mismatch for {variant}"
            );
        }
    }

    #[test]
    fn config_subjects_equal_subject_patterns() {
        let p = prefix("acp");
        for variant in AcpStream::ALL {
            let cfg = variant.config(&p);
            assert_eq!(
                cfg.subjects,
                variant.subject_patterns(&p),
                "config.subjects mismatch for {variant}"
            );
        }
    }

    #[test]
    fn all_configs_returns_six_configs() {
        let p = prefix("acp");
        let configs = AcpStream::all_configs(&p);
        assert_eq!(configs.len(), 6);
    }

    #[test]
    fn all_configs_names_are_distinct() {
        let p = prefix("acp");
        let configs = AcpStream::all_configs(&p);
        let mut names: Vec<_> = configs.iter().map(|c| c.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 6, "all config names must be unique");
    }

    #[test]
    fn display_matches_suffix_for_all_variants() {
        for variant in AcpStream::ALL {
            assert_eq!(format!("{variant}"), variant.suffix());
        }
    }
}
