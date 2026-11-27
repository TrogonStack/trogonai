/// Allow and deny lists over tool names. A pattern is an exact name or a prefix ending in `*`. An empty allow list
/// permits every tool; deny always wins.
#[derive(Debug, Clone, Default)]
pub struct ToolFilter {
    allow: Vec<String>,
    deny: Vec<String>,
}

fn pattern_matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

impl ToolFilter {
    pub fn new(allow: Vec<String>, deny: Vec<String>) -> Self {
        Self { allow, deny }
    }

    pub fn permits(&self, name: &str) -> bool {
        if self.deny.iter().any(|p| pattern_matches(p, name)) {
            return false;
        }
        self.allow.is_empty() || self.allow.iter().any(|p| pattern_matches(p, name))
    }

    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    /// Patterns that match none of `registered`, so a typo does not silently hide or expose tools.
    pub fn unmatched_patterns<'a>(&'a self, registered: &[String]) -> Vec<&'a str> {
        self.allow
            .iter()
            .chain(self.deny.iter())
            .filter(|p| !registered.iter().any(|n| pattern_matches(p, n)))
            .map(String::as_str)
            .collect()
    }
}

#[cfg(test)]
mod tests;
