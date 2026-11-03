#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSubjectPattern(String);

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("invalid server subscription pattern")]
pub struct ServerSubjectPatternError;

impl ServerSubjectPattern {
    pub fn new(input: &str) -> Result<Self, ServerSubjectPatternError> {
        let tokens: Vec<_> = input.split('.').collect();
        if tokens
            .iter()
            .any(|token| token.contains('>') || (token.contains('*') && *token != "*"))
        {
            return Err(ServerSubjectPatternError);
        }
        let concrete = tokens
            .iter()
            .map(|token| if *token == "*" { "principal" } else { *token })
            .collect::<Vec<_>>()
            .join(".");
        trogon_nats::subject_conformance::validate_published_subject(&concrete)
            .map_err(|_| ServerSubjectPatternError)?;
        Ok(Self(input.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
