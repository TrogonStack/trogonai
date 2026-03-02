pub mod agent {
    pub fn initialize(prefix: &str) -> String {
        format!("{}.agent.initialize", prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::agent;

    #[test]
    fn agent_initialize_formats_prefix_correctly() {
        assert_eq!(agent::initialize("acp"), "acp.agent.initialize");
    }

    #[test]
    fn agent_initialize_works_with_multi_part_prefix() {
        assert_eq!(
            agent::initialize("my.prefix"),
            "my.prefix.agent.initialize"
        );
    }

    /// Documents that `initialize()` performs no validation — an empty prefix
    /// produces a subject that starts with a dot.  Callers are responsible for
    /// passing a validated `AcpPrefix`.
    #[test]
    fn agent_initialize_empty_prefix_produces_dot_prefix() {
        assert_eq!(agent::initialize(""), ".agent.initialize");
    }
}
