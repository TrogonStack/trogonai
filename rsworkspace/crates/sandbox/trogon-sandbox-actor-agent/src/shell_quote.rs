/// Quotes `value` as one POSIX shell word.
///
/// Names are already parsed as unix names, so this is defence in depth rather than the only thing standing between a
/// name and the shell.
pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests;
