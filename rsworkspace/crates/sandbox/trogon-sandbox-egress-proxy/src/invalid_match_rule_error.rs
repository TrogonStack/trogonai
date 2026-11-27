#[derive(Debug, thiserror::Error)]
#[error("invalid regex {pattern:?} in match rule: {source}")]
pub struct InvalidMatchRuleError {
    pub(crate) pattern: String,
    pub(crate) source: regex::Error,
}
