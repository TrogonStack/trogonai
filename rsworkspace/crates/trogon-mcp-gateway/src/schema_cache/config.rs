use std::time::Duration;

#[derive(Clone, Debug)]
pub struct SchemaCacheConfig {
    pub enabled: bool,
    pub ttl: Duration,
    pub max_entries: usize,
}

impl Default for SchemaCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ttl: Duration::from_secs(120),
            max_entries: 10_000,
        }
    }
}

impl SchemaCacheConfig {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }
}
