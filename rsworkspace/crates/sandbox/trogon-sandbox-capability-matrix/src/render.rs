use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::Value;

use crate::constants::PLATFORM_DEPENDENT;
use crate::{Column, MatrixError};

/// Flattens a JSON value into `dotted.path -> leaf` pairs. Non-empty objects recurse; everything else is a
/// leaf.
fn flatten(prefix: &str, value: &Value, out: &mut BTreeMap<String, Value>) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, inner) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(&path, inner, out);
            }
        }
        other => {
            out.insert(prefix.to_owned(), other.clone());
        }
    }
}

/// The flattened profile of `column`, with each platform row collapsed into one entry so the set of paths
/// does not depend on the host.
fn flat_profile(column: &Column) -> Result<BTreeMap<String, Value>, MatrixError> {
    let value = serde_json::to_value(&column.profile).map_err(|source| MatrixError::Serialize {
        label: column.label,
        source,
    })?;
    let mut map = BTreeMap::new();
    flatten("", &value, &mut map);
    map.retain(|path, _| !column.is_platform_dependent(path));
    for row in column.platform_rows {
        map.insert((*row).to_owned(), Value::Null);
    }
    Ok(map)
}

fn render_cell(value: &Value) -> String {
    let text = match value {
        Value::Null => return "-".to_owned(),
        Value::String(s) => s.clone(),
        Value::Array(items) if items.is_empty() => return "none".to_owned(),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(_) => return "none".to_owned(),
        other => other.to_string(),
    };
    // A pipe would end the cell early.
    format!("`{}`", text.replace('|', "\\|"))
}

/// Renders the matrix as markdown: one table per top-level profile section, one row per flattened field,
/// one column per worker.
pub fn render(columns: &[Column]) -> Result<String, MatrixError> {
    let flat = columns.iter().map(flat_profile).collect::<Result<Vec<_>, _>>()?;

    let mut paths: Vec<&String> = flat.iter().flat_map(BTreeMap::keys).collect();
    paths.sort();
    paths.dedup();

    let mut sections: Vec<(&str, Vec<&String>)> = Vec::new();
    for path in paths {
        let section = path.split('.').next().unwrap_or(path);
        match sections.last_mut() {
            Some((name, rows)) if *name == section => rows.push(path),
            _ => sections.push((section, vec![path])),
        }
    }

    let mut out = String::new();
    for (section, rows) in sections {
        let _ = write!(out, "### `{section}`\n\n| Field |");
        for column in columns {
            let _ = write!(out, " {} |", column.label);
        }
        out.push_str("\n|---|");
        for _ in columns {
            out.push_str("---|");
        }
        out.push('\n');
        let prefix = format!("{section}.");
        for path in rows {
            let field = path.strip_prefix(&prefix).unwrap_or(path);
            let _ = write!(out, "| `{field}` |");
            for (column, map) in columns.iter().zip(&flat) {
                let cell = if column.is_platform_dependent(path) {
                    PLATFORM_DEPENDENT.to_owned()
                } else {
                    // Absent for a path only another worker has, such as a range this worker leaves unset.
                    map.get(path.as_str()).map_or_else(|| "-".to_owned(), render_cell)
                };
                let _ = write!(out, " {cell} |");
            }
            out.push('\n');
        }
        out.push('\n');
    }
    Ok(out.trim_end().to_owned() + "\n")
}

#[cfg(test)]
mod tests;
