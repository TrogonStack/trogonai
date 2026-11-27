use crate::ExtractKind;

/// Why a credential could not be located in a file. Errors name the rule, never the content.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum ExtractError {
    #[error("the extract rule ({0}) matched no credential in the file")]
    NoMatch(ExtractKind),
    #[error("the file is not valid JSON, which the json_key extract rule needs")]
    NotJson,
    #[error(
        "the extract pattern matched but capture group 1 did not take part in the match, so the credential cannot be located"
    )]
    CaptureAbsent,
}
