use std::fmt;

/// Why a TOML document did not parse, and where.
///
/// A `toml` error's `Display` quotes the line it failed on, which is whatever
/// the operator wrote there. This keeps the parser's message and the position,
/// never that line. A type error's message can still name the value it
/// rejected, so a setting that may carry a secret must be one serde cannot
/// reject by type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TomlSyntax {
    message: String,
    position: Option<(usize, usize)>,
}

impl TomlSyntax {
    pub fn of(error: &toml::de::Error, document: &str) -> Self {
        Self {
            message: error.message().to_owned(),
            position: error.span().map(|span| line_and_column(document, span.start)),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// The 1-based line the parser stopped on, when it reported one.
    pub fn line(&self) -> Option<usize> {
        self.position.map(|(line, _)| line)
    }

    /// The 1-based column, in characters, the parser stopped on.
    pub fn column(&self) -> Option<usize> {
        self.position.map(|(_, column)| column)
    }
}

impl fmt::Display for TomlSyntax {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.position {
            Some((line, column)) => write!(f, "{} at line {line}, column {column}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

fn line_and_column(document: &str, offset: usize) -> (usize, usize) {
    let before = document.get(..offset).unwrap_or(document);
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    (
        before.matches('\n').count() + 1,
        before[line_start..].chars().count() + 1,
    )
}

#[cfg(test)]
mod tests;
