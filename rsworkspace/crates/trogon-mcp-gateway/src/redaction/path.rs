use super::errors::RedactionError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PathSegment {
    Root,
    Field(String),
    Wildcard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedPath {
    pub segments: Vec<PathSegment>,
}

impl ParsedPath {
    pub fn parse(path: &str) -> Result<Self, RedactionError> {
        if path.is_empty() || !path.starts_with('$') {
            return Err(RedactionError::InvalidPath(format!(
                "path must start with $: {path}"
            )));
        }
        let mut segments = vec![PathSegment::Root];
        let rest = path.trim_start_matches('$');
        if rest.is_empty() {
            return Ok(Self { segments });
        }
        if !rest.starts_with('.') && !rest.starts_with('[') {
            return Err(RedactionError::InvalidPath(format!(
                "invalid path after $: {path}"
            )));
        }
        let mut chars = rest.chars().peekable();
        while chars.peek().is_some() {
            match chars.peek() {
                Some('.') => {
                    chars.next();
                    let key = read_field_name(&mut chars, path)?;
                    segments.push(PathSegment::Field(key));
                }
                Some('[') => {
                    chars.next();
                    if chars.next() != Some('*') {
                        return Err(RedactionError::InvalidPath(format!(
                            "only [*] array wildcards supported: {path}"
                        )));
                    }
                    if chars.next() != Some(']') {
                        return Err(RedactionError::InvalidPath(format!(
                            "unclosed array wildcard: {path}"
                        )));
                    }
                    segments.push(PathSegment::Wildcard);
                }
                Some(c) => {
                    return Err(RedactionError::InvalidPath(format!(
                        "unexpected character '{c}' in path: {path}"
                    )));
                }
                None => break,
            }
        }
        Ok(Self { segments })
    }
}

fn read_field_name(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, path: &str) -> Result<String, RedactionError> {
    let mut key = String::new();
    while let Some(c) = chars.peek() {
        if *c == '.' || *c == '[' {
            break;
        }
        key.push(*c);
        chars.next();
    }
    if key.is_empty() {
        return Err(RedactionError::InvalidPath(format!(
            "empty field name in path: {path}"
        )));
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_nested_field_path() {
        let parsed = ParsedPath::parse("$.params.user.email").expect("parse");
        assert_eq!(
            parsed.segments,
            vec![
                PathSegment::Root,
                PathSegment::Field("params".into()),
                PathSegment::Field("user".into()),
                PathSegment::Field("email".into()),
            ]
        );
    }

    #[test]
    fn parse_array_wildcard_path() {
        let parsed = ParsedPath::parse("$.result.rows[*].ssn").expect("parse");
        assert_eq!(
            parsed.segments,
            vec![
                PathSegment::Root,
                PathSegment::Field("result".into()),
                PathSegment::Field("rows".into()),
                PathSegment::Wildcard,
                PathSegment::Field("ssn".into()),
            ]
        );
    }
}
