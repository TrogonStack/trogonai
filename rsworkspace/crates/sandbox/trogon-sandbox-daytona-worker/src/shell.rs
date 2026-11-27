/// Quotes `value` as one POSIX shell word. Single quotes keep every byte literal; an embedded single quote closes the
/// word, is escaped, and reopens it.
pub(crate) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests;
