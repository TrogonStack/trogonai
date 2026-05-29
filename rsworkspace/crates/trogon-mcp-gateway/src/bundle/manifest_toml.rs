use serde_json::{Map, Value};

use super::errors::BundleLoadError;
use super::manifest::BundleManifest;

pub fn parse_toml(bytes: &[u8]) -> Result<Value, BundleLoadError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| BundleLoadError::ManifestParse(format!("manifest must be UTF-8: {error}")))?;
    if text.starts_with('\u{feff}') {
        return Err(BundleLoadError::ManifestParse(
            "manifest must not contain a UTF-8 BOM".into(),
        ));
    }
    let document = parse_document(text)?;
    Ok(document.into())
}

pub fn emit_toml(manifest: &BundleManifest) -> Result<String, BundleLoadError> {
    let value = serde_json::to_value(manifest)
        .map_err(|error| BundleLoadError::ManifestParse(error.to_string()))?;
    let document = TomlDocument::from_json(value)?;
    Ok(document.to_string())
}

#[derive(Debug, Default)]
struct TomlDocument {
    root: Map<String, Value>,
    tables: Vec<(String, Map<String, Value>)>,
    array_tables: Vec<(String, Map<String, Value>)>,
}

impl TomlDocument {
    fn from_json(value: Value) -> Result<Self, BundleLoadError> {
        let object = value
            .as_object()
            .ok_or_else(|| BundleLoadError::ManifestParse("manifest root must be a table".into()))?
            .clone();
        let mut document = Self::default();
        for (key, child) in object {
            match key.as_str() {
                "capabilities" | "signing" => {
                    let table = child
                        .as_object()
                        .ok_or_else(|| {
                            BundleLoadError::ManifestParse(format!("`{key}` must be a table"))
                        })?
                        .clone();
                    document.tables.push((key, table));
                }
                "programs" | "components" | "schemas" => {
                    let items = child
                        .as_array()
                        .ok_or_else(|| {
                            BundleLoadError::ManifestParse(format!("`{key}` must be an array"))
                        })?;
                    for item in items {
                        let table = item
                            .as_object()
                            .ok_or_else(|| {
                                BundleLoadError::ManifestParse(format!(
                                    "`{key}` entries must be tables"
                                ))
                            })?
                            .clone();
                        document.array_tables.push((key.to_string(), table));
                    }
                }
                _ => {
                    document.root.insert(key, child);
                }
            }
        }
        Ok(document)
    }

    fn into(self) -> Value {
        let mut root = self.root;
        for (name, table) in self.tables {
            root.insert(name, Value::Object(table));
        }
        for (name, table) in self.array_tables {
            root
                .entry(name)
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .expect("array table bucket")
                .push(Value::Object(table));
        }
        Value::Object(root)
    }

    fn to_string(&self) -> String {
        let mut out = String::new();
        emit_table(&mut out, &self.root, false);
        for (name, table) in &self.tables {
            out.push('\n');
            out.push('[');
            out.push_str(name);
            out.push(']');
            out.push('\n');
            emit_table(&mut out, table, false);
        }
        for (name, table) in &self.array_tables {
            out.push('\n');
            out.push('[');
            out.push('[');
            out.push_str(name);
            out.push(']');
            out.push(']');
            out.push('\n');
            emit_table(&mut out, table, false);
        }
        out
    }
}

fn emit_table(out: &mut String, table: &Map<String, Value>, inline: bool) {
    let mut keys: Vec<_> = table.keys().collect();
    keys.sort();
    for key in keys {
        let value = &table[key];
        if inline {
            if !out.is_empty() {
                out.push_str(", ");
            }
            out.push_str(key);
            out.push_str(" = ");
            emit_value(out, value);
        } else {
            out.push_str(key);
            out.push_str(" = ");
            emit_value(out, value);
            out.push('\n');
        }
    }
}

fn emit_value(out: &mut String, value: &Value) {
    match value {
        Value::String(text) => {
            out.push('"');
            for ch in text.chars() {
                match ch {
                    '\\' | '"' => {
                        out.push('\\');
                        out.push(ch);
                    }
                    _ => out.push(ch),
                }
            }
            out.push('"');
        }
        Value::Number(number) => out.push_str(&number.to_string()),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                emit_value(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            emit_table(out, map, true);
            out.push('}');
        }
        Value::Null => out.push_str("null"),
    }
}

fn parse_document(text: &str) -> Result<TomlDocument, BundleLoadError> {
    let mut document = TomlDocument::default();
    let mut current_table = TableTarget::Root;
    let mut pending_array_table: Option<String> = None;

    for (line_no, line) in collect_logical_lines(text)? {
        if line.starts_with("[[") {
            let name = parse_array_table_header(&line).map_err(|detail| parse_error(line_no, detail))?;
            pending_array_table = Some(name);
            current_table = TableTarget::PendingArray;
            continue;
        }
        if line.starts_with('[') {
            let name = parse_table_header(&line).map_err(|detail| parse_error(line_no, detail))?;
            pending_array_table = None;
            current_table = TableTarget::Named(name);
            continue;
        }

        let (key, value) = parse_assignment(&line).map_err(|detail| parse_error(line_no, detail))?;
        let value = parse_value(value).map_err(|detail| parse_error(line_no, detail))?;
        match current_table {
            TableTarget::Root => {
                document.root.insert(key, value);
            }
            TableTarget::Named(ref name) => {
                let table = document.table_mut(name);
                table.insert(key, value);
            }
            TableTarget::PendingArray => {
                let name = pending_array_table.clone().expect("array table name");
                document.array_tables.push((name, Map::from_iter([(key, value)])));
                current_table = TableTarget::LastArray;
            }
            TableTarget::LastArray => {
                let (_, table) = document
                    .array_tables
                    .last_mut()
                    .expect("active array table");
                table.insert(key, value);
            }
        }
    }

    Ok(document)
}

fn collect_logical_lines(text: &str) -> Result<Vec<(usize, String)>, BundleLoadError> {
    let mut logical = Vec::new();
    let mut buffer = String::new();
    let mut start_line = 0usize;

    for (line_no, raw_line) in text.lines().enumerate() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if buffer.is_empty() {
            start_line = line_no;
            buffer.push_str(line);
        } else {
            buffer.push(' ');
            buffer.push_str(line);
        }
        if statement_complete(&buffer) {
            logical.push((start_line, std::mem::take(&mut buffer)));
        }
    }

    if !buffer.is_empty() {
        return Err(parse_error(start_line, "unterminated statement".into()));
    }

    Ok(logical)
}

fn statement_complete(line: &str) -> bool {
    if line.starts_with('[') {
        return true;
    }
    if !line.contains('=') {
        return false;
    }
    bracket_depth(line) == 0
}

fn bracket_depth(line: &str) -> i32 {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for ch in line.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '[' if !in_string => depth += 1,
            ']' if !in_string => depth -= 1,
            _ => {}
        }
    }
    depth
}

enum TableTarget {
    Root,
    Named(String),
    PendingArray,
    LastArray,
}

impl TomlDocument {
    fn table_mut(&mut self, name: &str) -> &mut Map<String, Value> {
        if let Some(index) = self.tables.iter().position(|(existing, _)| existing == name) {
            return &mut self.tables[index].1;
        }
        self.tables.push((name.to_string(), Map::new()));
        let index = self.tables.len() - 1;
        &mut self.tables[index].1
    }
}

fn parse_error(line_no: usize, detail: String) -> BundleLoadError {
    BundleLoadError::ManifestParse(format!("line {}: {detail}", line_no + 1))
}

fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '#' if !in_string => return &line[..index],
            _ => {}
        }
    }
    line
}

fn parse_table_header(line: &str) -> Result<String, String> {
    let inner = line
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .ok_or_else(|| "expected [table] header".to_string())?;
    if inner.starts_with('[') {
        return Err("expected [table] header".into());
    }
    Ok(inner.trim().to_string())
}

fn parse_array_table_header(line: &str) -> Result<String, String> {
    let inner = line
        .strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
        .ok_or_else(|| "expected [[array.table]] header".to_string())?;
    Ok(inner.trim().to_string())
}

fn parse_assignment(line: &str) -> Result<(String, &str), String> {
    let (key, value) = line
        .split_once('=')
        .ok_or_else(|| "expected key = value".to_string())?;
    Ok((key.trim().to_string(), value.trim()))
}

fn parse_value(raw: &str) -> Result<Value, String> {
    if raw.starts_with('"') {
        return parse_string(raw);
    }
    if raw.starts_with('[') {
        return parse_array(raw);
    }
    if raw == "true" {
        return Ok(Value::Bool(true));
    }
    if raw == "false" {
        return Ok(Value::Bool(false));
    }
    if let Ok(number) = raw.parse::<i64>() {
        return Ok(Value::Number(number.into()));
    }
    Err(format!("unsupported value `{raw}`"))
}

fn parse_string(raw: &str) -> Result<Value, String> {
    let mut chars = raw.chars();
    if chars.next() != Some('"') {
        return Err("string must start with \"".into());
    }
    let mut out = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                other => {
                    return Err(format!("unsupported string escape `\\{other:?}`"));
                }
            },
            '"' => return Ok(Value::String(out)),
            other => out.push(other),
        }
    }
    Err("unterminated string".into())
}

fn parse_array(raw: &str) -> Result<Value, String> {
    let inner = raw
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .ok_or_else(|| "array must be enclosed in []".to_string())?;
    if inner.trim().is_empty() {
        return Ok(Value::Array(Vec::new()));
    }
    let mut items = Vec::new();
    for part in split_top_level(inner, ',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        items.push(parse_value(trimmed)?);
    }
    Ok(Value::Array(items))
}

fn split_top_level(input: &str, delimiter: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            ch if ch == delimiter && !in_string => {
                parts.push(input[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(input[start..].trim());
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_tables_and_arrays() {
        let toml = r#"
name = "acme/demo"
version = "1.0.0"

[signing]
nkey_pub = "UABTEST"

[[programs]]
id = "rule-a"
path = "policies/a.cel"
"#;
        let value = parse_toml(toml.as_bytes()).expect("parse");
        assert_eq!(value["name"], "acme/demo");
        assert_eq!(value["signing"]["nkey_pub"], "UABTEST");
        assert_eq!(value["programs"][0]["id"], "rule-a");
    }
}
