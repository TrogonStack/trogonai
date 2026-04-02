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
        Config {
            name: self.stream_name(prefix),
            subjects: self.subject_patterns(prefix),
            storage: StorageType::File,
            retention: RetentionPolicy::Limits,
            max_age: DEFAULT_STREAM_MAX_AGE,
            discard: DiscardPolicy::Old,
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
