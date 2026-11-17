use crate::PythonString;
use crate::python_urllib::UrlText;

/// Why a [`crate::RemoteSchemaRetriever`] could not open or read a remote schema: the class name and the `str()`
/// of the exception Python's `urlopen` raises in its place, such as `URLError` and
/// `<urlopen error [Errno 61] Connection refused>`.
///
/// The text is a [`PythonString`], as Python's is: a proxy URL read from a variable that is not UTF-8 holds lone
/// surrogates, and an error quoting it, such as `<urlopen error unknown url type: x\udcff>`, keeps them.
///
/// It displays as `{class}: {text}`, each lone surrogate of the text written as U+FFFD.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{}: {}", .class, .text.to_str_lossy())]
pub struct RemoteSchemaError {
    class: String,
    text: PythonString,
}

impl RemoteSchemaError {
    pub fn new(class: impl Into<String>, text: impl Into<PythonString>) -> Self {
        Self {
            class: class.into(),
            text: text.into(),
        }
    }

    /// The exception class name, such as `URLError`, `HTTPError` or `ValueError`.
    pub fn class(&self) -> &str {
        &self.class
    }

    /// The exception text, as Python's `str(error)` spells it.
    pub fn text(&self) -> &PythonString {
        &self.text
    }
}

pub(crate) fn failure(class: &str, text: impl Into<PythonString>) -> RemoteSchemaError {
    RemoteSchemaError::new(class, text)
}

/// `URLError(reason)`, whose `str` keeps a lone surrogate of the reason.
pub(crate) fn url_error<T: UrlText + ?Sized>(reason: &T) -> RemoteSchemaError {
    failure("URLError", python_concat(&[b"<urlopen error ", reason.encoded(), b">"]))
}

/// The text the encodings of `parts` spell one after another.
pub(crate) fn python_concat(parts: &[&[u8]]) -> PythonString {
    PythonString::from_encoded(parts.concat())
}

#[cfg(test)]
#[path = "remote_schema_error_tests.rs"]
mod tests;
