use serde_json::Value;

/// A JSON object with the given members.
pub(crate) fn object<const N: usize>(entries: [(&str, Value); N]) -> Value {
    Value::Object(
        entries
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}

#[cfg(test)]
mod tests;
