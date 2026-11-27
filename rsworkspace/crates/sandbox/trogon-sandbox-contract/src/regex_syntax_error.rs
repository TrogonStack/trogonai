/// Why a pattern is not a valid regular expression in the ECMAScript dialect the wire uses.
///
/// Each message reads the way a JavaScript engine words the same refusal.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum RegexSyntaxError {
    #[error("Unterminated group")]
    UnterminatedGroup,
    #[error("Unmatched ')'")]
    UnmatchedParenthesis,
    #[error("Nothing to repeat")]
    NothingToRepeat,
    #[error("Unterminated character class")]
    UnterminatedClass,
    #[error("Range out of order in character class")]
    ClassRangeOutOfOrder,
    #[error("numbers out of order in {{}} quantifier")]
    QuantifierOutOfOrder,
    #[error("\\ at end of pattern")]
    TrailingBackslash,
    #[error("Invalid group")]
    InvalidGroup,
    #[error("Invalid flag group")]
    InvalidFlagGroup,
    #[error("Repeated flag in flag group")]
    RepeatedFlag,
    #[error("Invalid quantifier")]
    InvalidQuantifier,
    #[error("Invalid escape")]
    InvalidEscape,
    #[error("Invalid capture group name")]
    InvalidGroupName,
    #[error("Invalid named reference")]
    InvalidNamedReference,
    #[error("Invalid named capture referenced")]
    InvalidNamedCapture,
    #[error("Duplicate capture group name")]
    DuplicateGroupName,
    #[error("Invalid regular expression")]
    Unsupported,
}
