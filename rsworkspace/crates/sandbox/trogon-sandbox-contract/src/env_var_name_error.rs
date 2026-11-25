/// A string is not an environment variable name [`EnvVarName`](crate::EnvVarName) accepts.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum EnvVarNameError {
    #[error(
        "environment variable name '{0}' must start with a letter or underscore and contain only letters, digits and underscores"
    )]
    Invalid(String),
}
