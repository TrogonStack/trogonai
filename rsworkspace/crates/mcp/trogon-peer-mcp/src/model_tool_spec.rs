use serde_json::{Map, Value};

use crate::argument_issue::ArgumentIssue;

pub(crate) mod constants;

/// What a model tool accepts, so a rejected call can name the field and show a call that works.
#[derive(Debug)]
pub(crate) struct ModelToolSpec {
    pub(crate) name: &'static str,
    pub(crate) fields: &'static [FieldSpec],
    pub(crate) example: &'static str,
}

#[derive(Debug)]
pub(crate) struct FieldSpec {
    pub(crate) name: &'static str,
    pub(crate) required: bool,
    pub(crate) kind: FieldKind,
    /// Field names models use instead of this one.
    pub(crate) aliases: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum FieldKind {
    Text,
    Count,
}

impl ModelToolSpec {
    /// The first field-level problem with these arguments.
    pub(crate) fn check(&self, arguments: &Map<String, Value>) -> Result<(), ArgumentIssue> {
        if let Some(unknown) = arguments.keys().find(|key| self.field(key).is_none()) {
            let instead = self
                .fields
                .iter()
                .find(|field| field.aliases.contains(&unknown.as_str()))
                .map(|field| field.name);
            return Err(ArgumentIssue::UnknownField {
                field: unknown.clone(),
                instead,
            });
        }
        for field in self.fields {
            match (arguments.get(field.name), field.kind) {
                (None | Some(Value::Null), _) if field.required => {
                    return Err(ArgumentIssue::MissingField(field.name));
                }
                (None | Some(Value::Null), _) | (Some(Value::String(_)), FieldKind::Text) => {}
                (Some(value), FieldKind::Count) if value.as_u64().is_some_and(|n| u32::try_from(n).is_ok()) => {}
                (Some(_), kind) => {
                    return Err(ArgumentIssue::WrongType {
                        field: field.name,
                        expected: match kind {
                            FieldKind::Text => "a string",
                            FieldKind::Count => "a non-negative integer",
                        },
                    });
                }
            }
        }
        Ok(())
    }

    fn field(&self, name: &str) -> Option<&FieldSpec> {
        self.fields.iter().find(|field| field.name == name)
    }

    /// The tool's required and optional fields, as one sentence.
    pub(crate) fn usage(&self) -> String {
        let names = |required: bool| {
            self.fields
                .iter()
                .filter(move |field| field.required == required)
                .map(|field| field.name)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let (required, optional) = (names(true), names(false));
        let required = if required.is_empty() {
            "no required fields"
        } else {
            required.as_str()
        };
        if optional.is_empty() {
            format!("{} takes {required}.", self.name)
        } else {
            format!("{} takes {required} (optional: {optional}).", self.name)
        }
    }
}

#[cfg(test)]
mod tests;
