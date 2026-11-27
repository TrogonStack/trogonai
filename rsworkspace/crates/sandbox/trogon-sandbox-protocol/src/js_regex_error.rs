/// An extract pattern that passed the syntax check but the regex engine cannot compile.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum JsRegexError {
    #[error("Invalid regular expression")]
    Unsupported,
}
