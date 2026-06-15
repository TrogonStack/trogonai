//! Shared YAML-frontmatter markdown template helpers for custom commands and skills.

/// Split YAML frontmatter from a markdown template body.
pub fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Some(("", content));
    }
    let rest = trimmed.strip_prefix("---")?;
    let rest = rest.strip_prefix('\n').or_else(|| rest.strip_prefix("\r\n"))?;
    let end = rest.find("\n---")?;
    let (yaml, body) = rest.split_at(end);
    let body = body.strip_prefix("\n---").unwrap_or(body);
    let body = body.strip_prefix('\n').or_else(|| body.strip_prefix("\r\n")).unwrap_or(body);
    Some((yaml, body))
}

/// Replace `$ARGUMENTS` and positional `$1`, `$2`, … in a command template.
pub fn substitute_args(template: &str, args: &str) -> String {
    let trimmed = args.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    let mut out = template.to_string();
    for i in (1..=parts.len()).rev() {
        out = out.replace(&format!("${i}"), parts[i - 1]);
    }
    out.replace("$ARGUMENTS", trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_extracts_yaml_and_body() {
        let content = "---\ndescription: x\n---\nBody here\n";
        let (yaml, body) = split_frontmatter(content).unwrap();
        assert_eq!(yaml, "description: x");
        assert_eq!(body, "Body here\n");
    }

    #[test]
    fn split_frontmatter_without_marker_returns_whole_content() {
        let (yaml, body) = split_frontmatter("No frontmatter").unwrap();
        assert_eq!(yaml, "");
        assert_eq!(body, "No frontmatter");
    }

    #[test]
    fn substitute_arguments_and_positional() {
        assert_eq!(
            substitute_args("All: $ARGUMENTS", "hello world"),
            "All: hello world"
        );
        assert_eq!(
            substitute_args("First=$1 second=$2 rest=$ARGUMENTS", "a b c d"),
            "First=a second=b rest=a b c d"
        );
        assert_eq!(substitute_args("No args: $1", ""), "No args: $1");
    }
}
