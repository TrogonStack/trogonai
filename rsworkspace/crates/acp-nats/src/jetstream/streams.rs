use async_nats::jetstream::stream::Config;

use crate::acp_prefix::AcpPrefix;
use crate::nats::AcpStream;

pub fn notifications_stream_name(prefix: &AcpPrefix) -> String {
    AcpStream::Notifications.stream_name(prefix)
}

pub fn responses_stream_name(prefix: &AcpPrefix) -> String {
    AcpStream::Responses.stream_name(prefix)
}

pub fn commands_stream_name(prefix: &AcpPrefix) -> String {
    AcpStream::Commands.stream_name(prefix)
}

pub fn global_stream_name(prefix: &AcpPrefix) -> String {
    AcpStream::Global.stream_name(prefix)
}

pub fn global_ext_stream_name(prefix: &AcpPrefix) -> String {
    AcpStream::GlobalExt.stream_name(prefix)
}

pub fn all_configs(prefix: &AcpPrefix) -> [Config; 6] {
    AcpStream::all_configs(prefix)
}

#[cfg(test)]
mod tests {
    use async_nats::jetstream::stream::{DiscardPolicy, RetentionPolicy, StorageType};

    use crate::acp_prefix::AcpPrefix;
    use crate::constants::DEFAULT_STREAM_MAX_AGE;
    use crate::nats::AcpStream;

    use super::*;

    fn p(s: &str) -> AcpPrefix {
        AcpPrefix::new(s).expect("test prefix")
    }

    #[test]
    fn stream_names_use_uppercase_prefix() {
        let prefix = p("acp");
        assert_eq!(AcpStream::Commands.stream_name(&prefix), "ACP_COMMANDS");
        assert_eq!(AcpStream::Responses.stream_name(&prefix), "ACP_RESPONSES");
        assert_eq!(AcpStream::ClientOps.stream_name(&prefix), "ACP_CLIENT_OPS");
        assert_eq!(AcpStream::Notifications.stream_name(&prefix), "ACP_NOTIFICATIONS");
        assert_eq!(AcpStream::Global.stream_name(&prefix), "ACP_GLOBAL");
        assert_eq!(AcpStream::GlobalExt.stream_name(&prefix), "ACP_GLOBAL_EXT");
    }

    #[test]
    fn stream_names_with_custom_prefix() {
        let prefix = p("myapp");
        assert_eq!(AcpStream::Commands.stream_name(&prefix), "MYAPP_COMMANDS");
        assert_eq!(AcpStream::Responses.stream_name(&prefix), "MYAPP_RESPONSES");
    }

    #[test]
    fn stream_names_normalize_dots_to_underscores() {
        let prefix = p("my.multi.part");
        assert_eq!(AcpStream::Commands.stream_name(&prefix), "MY_MULTI_PART_COMMANDS");
        assert_eq!(AcpStream::Global.stream_name(&prefix), "MY_MULTI_PART_GLOBAL");
    }

    #[test]
    fn commands_subjects_are_session_scoped_only() {
        let config = AcpStream::Commands.config(&p("acp"));
        assert!(!config.subjects.contains(&"acp.agent.>".to_string()));
        assert!(config.subjects.contains(&"acp.session.*.agent.prompt".to_string()));
        assert!(config.subjects.contains(&"acp.session.*.agent.fork".to_string()));
        assert!(config.subjects.contains(&"acp.session.*.agent.close".to_string()));
    }

    #[test]
    fn commands_excludes_ext_subjects() {
        let config = AcpStream::Commands.config(&p("acp"));
        assert!(!config.subjects.iter().any(|s| s.contains("ext")));
    }

    #[test]
    fn responses_subjects() {
        let config = AcpStream::Responses.config(&p("acp"));
        assert!(
            config
                .subjects
                .contains(&"acp.session.*.agent.prompt.response.>".to_string())
        );
        assert!(config.subjects.contains(&"acp.session.*.agent.ext.ready".to_string()));
        assert!(config.subjects.contains(&"acp.session.*.agent.cancelled".to_string()));
    }

    #[test]
    fn client_ops_subjects() {
        let config = AcpStream::ClientOps.config(&p("acp"));
        assert_eq!(config.subjects, vec!["acp.session.*.client.>"]);
    }

    #[test]
    fn notifications_subjects() {
        let config = AcpStream::Notifications.config(&p("acp"));
        assert_eq!(config.subjects, vec!["acp.session.*.agent.update.>"]);
    }

    #[test]
    fn all_streams_use_file_storage() {
        let prefix = p("acp");
        for stream in AcpStream::ALL {
            assert_eq!(stream.config(&prefix).storage, StorageType::File);
        }
    }

    #[test]
    fn all_streams_have_30_day_max_age() {
        let prefix = p("acp");
        for stream in AcpStream::ALL {
            assert_eq!(stream.config(&prefix).max_age, DEFAULT_STREAM_MAX_AGE);
        }
    }

    #[test]
    fn all_streams_use_discard_old() {
        for config in all_configs(&p("acp")) {
            assert_eq!(config.discard, DiscardPolicy::Old);
        }
    }

    #[test]
    fn all_streams_use_limits_retention() {
        for config in all_configs(&p("acp")) {
            assert_eq!(config.retention, RetentionPolicy::Limits);
        }
    }

    #[test]
    fn notifications_stream_name_formats_correctly() {
        assert_eq!(notifications_stream_name(&p("acp")), "ACP_NOTIFICATIONS");
        assert_eq!(notifications_stream_name(&p("myapp")), "MYAPP_NOTIFICATIONS");
    }

    #[test]
    fn responses_stream_name_formats_correctly() {
        assert_eq!(responses_stream_name(&p("acp")), "ACP_RESPONSES");
        assert_eq!(responses_stream_name(&p("myapp")), "MYAPP_RESPONSES");
    }

    #[test]
    fn commands_stream_name_formats_correctly() {
        assert_eq!(commands_stream_name(&p("acp")), "ACP_COMMANDS");
        assert_eq!(commands_stream_name(&p("myapp")), "MYAPP_COMMANDS");
    }

    #[test]
    fn global_stream_name_formats_correctly() {
        assert_eq!(global_stream_name(&p("acp")), "ACP_GLOBAL");
        assert_eq!(global_stream_name(&p("myapp")), "MYAPP_GLOBAL");
    }

    #[test]
    fn global_subjects_include_expected() {
        let config = AcpStream::Global.config(&p("acp"));
        assert!(config.subjects.contains(&"acp.agent.initialize".to_string()));
        assert!(config.subjects.contains(&"acp.agent.authenticate".to_string()));
        assert!(config.subjects.contains(&"acp.agent.logout".to_string()));
        assert!(config.subjects.contains(&"acp.agent.session.new".to_string()));
    }

    #[test]
    fn global_excludes_session_list_and_ext() {
        let config = AcpStream::Global.config(&p("acp"));
        assert!(!config.subjects.contains(&"acp.agent.session.list".to_string()));
        assert!(!config.subjects.contains(&"acp.agent.>".to_string()));
        assert!(!config.subjects.contains(&"acp.agent.ext.>".to_string()));
    }

    #[test]
    fn global_ext_subjects() {
        let config = AcpStream::GlobalExt.config(&p("acp"));
        assert_eq!(config.subjects, vec!["acp.agent.ext.>"]);
    }

    #[test]
    fn global_ext_stream_name_formats_correctly() {
        assert_eq!(global_ext_stream_name(&p("acp")), "ACP_GLOBAL_EXT");
        assert_eq!(global_ext_stream_name(&p("myapp")), "MYAPP_GLOBAL_EXT");
    }

    #[test]
    fn all_configs_returns_six_streams() {
        assert_eq!(all_configs(&p("acp")).len(), 6);
    }

    fn nats_pattern_matches(pattern: &str, subject: &str) -> bool {
        let pattern_tokens: Vec<&str> = pattern.split('.').collect();
        let subject_tokens: Vec<&str> = subject.split('.').collect();

        let mut pi = 0;
        let mut si = 0;
        while pi < pattern_tokens.len() && si < subject_tokens.len() {
            match pattern_tokens[pi] {
                ">" => return true,
                "*" => {
                    pi += 1;
                    si += 1;
                }
                token => {
                    if token != subject_tokens[si] {
                        return false;
                    }
                    pi += 1;
                    si += 1;
                }
            }
        }
        pi == pattern_tokens.len() && si == subject_tokens.len()
    }

    #[test]
    fn session_list_not_captured_by_any_stream() {
        let prefix = p("acp");
        let session_list_subject = "acp.agent.session.list";
        for stream in AcpStream::ALL {
            for pattern in stream.subject_patterns(&prefix) {
                assert!(
                    !nats_pattern_matches(&pattern, session_list_subject),
                    "stream {} pattern {pattern} would capture session.list",
                    stream
                );
            }
        }
    }

    #[test]
    fn no_subject_overlaps_between_streams() {
        let configs = all_configs(&p("acp"));
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
