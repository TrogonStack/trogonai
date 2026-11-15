//! The choices and callbacks of one tool's input validation, keyed by the JSON Pointer of the schema node
//! they apply to.

use crate::{JsonObject, SchemaCallback, SchemaChoice, SchemaPointer, StringCheckKind};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ValidationChoicesError {
    #[error("`{pointer}` has two {kind} choices")]
    DuplicateChoice { pointer: SchemaPointer, kind: &'static str },
    #[error("`{pointer}` has two callbacks")]
    DuplicateCallback { pointer: SchemaPointer },
    #[error("`{pointer}` orders the string checks without naming each of them once")]
    IncompleteCheckOrder { pointer: SchemaPointer },
}

#[derive(Clone, Debug, Default)]
pub struct ValidationChoicesOptions {
    pub choices: Vec<(SchemaPointer, SchemaChoice)>,
    pub callbacks: Vec<(SchemaPointer, SchemaCallback)>,
}

/// The choices and callbacks of one tool's input validation.
#[derive(Clone, Debug, Default)]
pub struct ValidationChoices {
    choices: Vec<(SchemaPointer, SchemaChoice)>,
    callbacks: Vec<(SchemaPointer, Arc<SchemaCallback>)>,
}

/// A tool's input schema with the choices written into it as internal keywords, and the callbacks they
/// index. Only the validation sees it.
pub(crate) struct PreparedSchema {
    pub(crate) schema: Value,
    pub(crate) callbacks: Vec<Arc<SchemaCallback>>,
}

impl ValidationChoices {
    pub fn new(options: ValidationChoicesOptions) -> Result<Self, ValidationChoicesError> {
        let ValidationChoicesOptions { choices, callbacks } = options;
        for (position, (pointer, choice)) in choices.iter().enumerate() {
            if choices[..position]
                .iter()
                .any(|(other, earlier)| other == pointer && earlier.kind() == choice.kind())
            {
                return Err(ValidationChoicesError::DuplicateChoice {
                    pointer: pointer.clone(),
                    kind: choice.kind(),
                });
            }
            if let SchemaChoice::StringCheckOrder(order) = choice {
                let complete = [
                    StringCheckKind::Minimum,
                    StringCheckKind::Maximum,
                    StringCheckKind::Format,
                ]
                .iter()
                .all(|kind| order.iter().filter(|given| *given == kind).count() == 1)
                    && order.len() == 3;
                if !complete {
                    return Err(ValidationChoicesError::IncompleteCheckOrder {
                        pointer: pointer.clone(),
                    });
                }
            }
        }
        for (position, (pointer, _)) in callbacks.iter().enumerate() {
            if callbacks[..position].iter().any(|(other, _)| other == pointer) {
                return Err(ValidationChoicesError::DuplicateCallback {
                    pointer: pointer.clone(),
                });
            }
        }
        Ok(Self {
            choices,
            callbacks: callbacks
                .into_iter()
                .map(|(pointer, callback)| (pointer, Arc::new(callback)))
                .collect(),
        })
    }

    /// `schema` with the choices written in, or the first pointer that does not name an object of it.
    pub(crate) fn prepare(&self, schema: &JsonObject) -> Result<PreparedSchema, SchemaPointer> {
        let mut prepared = Value::Object(schema.clone());
        for (pointer, choice) in &self.choices {
            match prepared.pointer_mut(pointer.as_str()) {
                Some(Value::Object(target)) => choice.annotate(target),
                _ => return Err(pointer.clone()),
            }
        }
        let mut callbacks = Vec::with_capacity(self.callbacks.len());
        for (pointer, callback) in &self.callbacks {
            match prepared.pointer_mut(pointer.as_str()) {
                Some(Value::Object(target)) => {
                    target.insert("x-trogon-callback".into(), Value::from(callbacks.len() as u64));
                    callbacks.push(Arc::clone(callback));
                }
                _ => return Err(pointer.clone()),
            }
        }
        Ok(PreparedSchema {
            schema: prepared,
            callbacks,
        })
    }
}
