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
        format!("{}_{}", prefix.as_str().to_uppercase().replace('.', "_"), self.suffix())
    }

    pub fn subject_patterns(&self, prefix: &AcpPrefix) -> Vec<String> {
        let p = prefix.as_str();
        match self {
            Self::Commands => vec![
                format!("{p}.v1.session.*.agent.prompt"),
                format!("{p}.v1.session.*.agent.cancel"),
                format!("{p}.v1.session.*.agent.load"),
                format!("{p}.v1.session.*.agent.set_mode"),
                format!("{p}.v1.session.*.agent.set_config_option"),
                format!("{p}.v1.session.*.agent.fork"),
                format!("{p}.v1.session.*.agent.resume"),
                format!("{p}.v1.session.*.agent.close"),
                format!("{p}.v1.session.*.agent.delete"),
            ],
            Self::Responses => vec![
                format!("{p}.v1.session.*.agent.response"),
                format!("{p}.v1.session.*.agent.ext.ready"),
                format!("{p}.v1.session.*.agent.cancelled"),
            ],
            Self::ClientOps => vec![format!("{p}.v1.session.*.client.>")],
            Self::Notifications => vec![format!("{p}.v1.session.*.agent.update")],
            Self::Global => vec![
                format!("{p}.v1.global.agent.initialize"),
                format!("{p}.v1.global.agent.authenticate"),
                format!("{p}.v1.global.agent.logout"),
                format!("{p}.v1.global.agent.session.new"),
            ],
            Self::GlobalExt => vec![format!("{p}.v1.global.agent.ext.>")],
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
