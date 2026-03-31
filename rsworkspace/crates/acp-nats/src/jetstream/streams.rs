use async_nats::jetstream::stream::{Config, DiscardPolicy, RetentionPolicy, StorageType};

use crate::constants::DEFAULT_STREAM_MAX_AGE;

fn stream_name(prefix: &str, suffix: &str) -> String {
    format!("{}_{}", prefix.to_uppercase(), suffix)
}

pub fn notifications_stream_name(prefix: &str) -> String {
    stream_name(prefix, "NOTIFICATIONS")
}

pub fn responses_stream_name(prefix: &str) -> String {
    stream_name(prefix, "RESPONSES")
}

pub fn commands_stream_name(prefix: &str) -> String {
    stream_name(prefix, "COMMANDS")
}

pub fn commands_config(prefix: &str) -> Config {
    Config {
        name: stream_name(prefix, "COMMANDS"),
        subjects: vec![
            format!("{prefix}.session.*.agent.prompt"),
            format!("{prefix}.session.*.agent.cancel"),
            format!("{prefix}.session.*.agent.load"),
            format!("{prefix}.session.*.agent.set_mode"),
            format!("{prefix}.session.*.agent.set_config_option"),
            format!("{prefix}.session.*.agent.set_model"),
            format!("{prefix}.session.*.agent.fork"),
            format!("{prefix}.session.*.agent.resume"),
            format!("{prefix}.session.*.agent.close"),
        ],
        storage: StorageType::File,
        retention: RetentionPolicy::Limits,
        max_age: DEFAULT_STREAM_MAX_AGE,
        discard: DiscardPolicy::Old,
        ..Default::default()
    }
}

pub fn responses_config(prefix: &str) -> Config {
    Config {
        name: stream_name(prefix, "RESPONSES"),
        subjects: vec![
            format!("{prefix}.session.*.agent.prompt.response.>"),
            format!("{prefix}.session.*.agent.response.>"),
            format!("{prefix}.session.*.agent.ext.ready"),
            format!("{prefix}.session.*.agent.cancelled"),
        ],
        storage: StorageType::File,
        retention: RetentionPolicy::Limits,
        max_age: DEFAULT_STREAM_MAX_AGE,
        discard: DiscardPolicy::Old,
        ..Default::default()
    }
}

pub fn client_ops_config(prefix: &str) -> Config {
    Config {
        name: stream_name(prefix, "CLIENT_OPS"),
        subjects: vec![format!("{prefix}.session.*.client.>")],
        storage: StorageType::File,
        retention: RetentionPolicy::Limits,
        max_age: DEFAULT_STREAM_MAX_AGE,
        discard: DiscardPolicy::Old,
        ..Default::default()
    }
}

pub fn notifications_config(prefix: &str) -> Config {
    Config {
        name: stream_name(prefix, "NOTIFICATIONS"),
        subjects: vec![format!("{prefix}.session.*.agent.update.>")],
        storage: StorageType::File,
        retention: RetentionPolicy::Limits,
        max_age: DEFAULT_STREAM_MAX_AGE,
        discard: DiscardPolicy::Old,
        ..Default::default()
    }
}

pub fn all_configs(prefix: &str) -> [Config; 4] {
    [
        commands_config(prefix),
        responses_config(prefix),
        client_ops_config(prefix),
        notifications_config(prefix),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_names_use_uppercase_prefix() {
        assert_eq!(commands_config("acp").name, "ACP_COMMANDS");
        assert_eq!(responses_config("acp").name, "ACP_RESPONSES");
        assert_eq!(client_ops_config("acp").name, "ACP_CLIENT_OPS");
        assert_eq!(notifications_config("acp").name, "ACP_NOTIFICATIONS");
    }

    #[test]
    fn stream_names_with_custom_prefix() {
        assert_eq!(commands_config("myapp").name, "MYAPP_COMMANDS");
        assert_eq!(responses_config("myapp").name, "MYAPP_RESPONSES");
    }

    #[test]
    fn commands_subjects_are_session_scoped_only() {
        let config = commands_config("acp");
        assert!(!config.subjects.contains(&"acp.agent.>".to_string()));
        assert!(
            config
                .subjects
                .contains(&"acp.session.*.agent.prompt".to_string())
        );
        assert!(
            config
                .subjects
                .contains(&"acp.session.*.agent.fork".to_string())
        );
        assert!(
            config
                .subjects
                .contains(&"acp.session.*.agent.close".to_string())
        );
    }

    #[test]
    fn commands_excludes_ext_subjects() {
        let config = commands_config("acp");
        assert!(!config.subjects.iter().any(|s| s.contains("ext")));
    }

    #[test]
    fn responses_subjects() {
        let config = responses_config("acp");
        assert!(
            config
                .subjects
                .contains(&"acp.session.*.agent.prompt.response.>".to_string())
        );
        assert!(
            config
                .subjects
                .contains(&"acp.session.*.agent.ext.ready".to_string())
        );
        assert!(
            config
                .subjects
                .contains(&"acp.session.*.agent.cancelled".to_string())
        );
    }

    #[test]
    fn client_ops_subjects() {
        let config = client_ops_config("acp");
        assert_eq!(config.subjects, vec!["acp.session.*.client.>"]);
    }

    #[test]
    fn notifications_subjects() {
        let config = notifications_config("acp");
        assert_eq!(config.subjects, vec!["acp.session.*.agent.update.>"]);
    }

    #[test]
    fn all_streams_use_file_storage() {
        assert_eq!(commands_config("acp").storage, StorageType::File);
        assert_eq!(responses_config("acp").storage, StorageType::File);
        assert_eq!(client_ops_config("acp").storage, StorageType::File);
        assert_eq!(notifications_config("acp").storage, StorageType::File);
    }

    #[test]
    fn all_streams_have_30_day_max_age() {
        assert_eq!(commands_config("acp").max_age, DEFAULT_STREAM_MAX_AGE);
        assert_eq!(responses_config("acp").max_age, DEFAULT_STREAM_MAX_AGE);
        assert_eq!(client_ops_config("acp").max_age, DEFAULT_STREAM_MAX_AGE);
        assert_eq!(notifications_config("acp").max_age, DEFAULT_STREAM_MAX_AGE);
    }

    #[test]
    fn all_streams_use_discard_old() {
        for config in all_configs("acp") {
            assert_eq!(config.discard, DiscardPolicy::Old);
        }
    }

    #[test]
    fn all_streams_use_limits_retention() {
        for config in all_configs("acp") {
            assert_eq!(config.retention, RetentionPolicy::Limits);
        }
    }

    #[test]
    fn notifications_stream_name_formats_correctly() {
        assert_eq!(notifications_stream_name("acp"), "ACP_NOTIFICATIONS");
        assert_eq!(notifications_stream_name("myapp"), "MYAPP_NOTIFICATIONS");
    }

    #[test]
    fn responses_stream_name_formats_correctly() {
        assert_eq!(responses_stream_name("acp"), "ACP_RESPONSES");
        assert_eq!(responses_stream_name("myapp"), "MYAPP_RESPONSES");
    }

    #[test]
    fn commands_stream_name_formats_correctly() {
        assert_eq!(commands_stream_name("acp"), "ACP_COMMANDS");
        assert_eq!(commands_stream_name("myapp"), "MYAPP_COMMANDS");
    }

    #[test]
    fn all_configs_returns_four_streams() {
        assert_eq!(all_configs("acp").len(), 4);
    }

    #[test]
    fn no_subject_overlaps_between_streams() {
        let configs = all_configs("acp");
        let all_subjects: Vec<&String> = configs.iter().flat_map(|c| &c.subjects).collect();

        for (i, a) in all_subjects.iter().enumerate() {
            for (j, b) in all_subjects.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "duplicate subject: {a}");
                }
            }
        }
    }
}
