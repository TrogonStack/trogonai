use crate::RegexSyntaxError;

/// A string is not an extract pattern [`ExtractPattern`](crate::ExtractPattern) accepts.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum ExtractPatternError {
    #[error(
        "extract is not a valid regular expression: Invalid regular expression: /{}/: {reason}",
        escape_slashes(pattern)
    )]
    Invalid { pattern: String, reason: RegexSyntaxError },
    #[error(
        "extract must contain at least one capturing group - group 1 is the credential value to mask (e.g. \"token:\\s*(\\S+)\")."
    )]
    WithoutCapture,
}

/// Escapes every `/` that is not already escaped, the way a regular expression literal shows its source.
fn escape_slashes(source: &str) -> String {
    let mut shown = String::with_capacity(source.len());
    let mut escaped = false;
    for character in source.chars() {
        match character {
            '/' if !escaped => shown.push_str("\\/"),
            _ => shown.push(character),
        }
        escaped = character == '\\' && !escaped;
    }
    shown
}
