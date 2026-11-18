use crate::{WireMessageDescriptor, WirePreflightError, schema_data::SchemaData};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Debug)]
pub struct WireMessageMapProfile {
    schema: Arc<SchemaData>,
    entries: BTreeSet<String>,
}

impl WireMessageMapProfile {
    pub(crate) fn new(schema: Arc<SchemaData>, entries: &[(&str, &str)]) -> Result<Self, WireMessageMapProfileError> {
        if entries.is_empty() || entries.len() > 32 {
            return Err(WireMessageMapProfileError::Invalid);
        }
        let mut names = BTreeSet::new();
        for (entry, value) in entries {
            if entry.is_empty()
                || entry.len() > 1024
                || value.is_empty()
                || value.len() > 1024
                || !names.insert((*entry).to_owned())
            {
                return Err(WireMessageMapProfileError::Invalid);
            }
            let shape = schema.messages.get(*entry).ok_or(WireMessageMapProfileError::Invalid)?;
            let key = shape.fields.get(&1).ok_or(WireMessageMapProfileError::Invalid)?;
            let field = shape.fields.get(&2).ok_or(WireMessageMapProfileError::Invalid)?;
            if !shape.map_entry
                || shape.has_oneof
                || shape.fields.len() != 2
                || key.kind != "TYPE_STRING"
                || key.repeated
                || key.required
                || key.oneof.is_some()
                || key.type_name.is_some()
                || field.kind != "TYPE_MESSAGE"
                || field.repeated
                || field.required
                || field.oneof.is_some()
                || field.type_name.as_deref() != Some(*value)
            {
                return Err(WireMessageMapProfileError::Invalid);
            }
        }
        Ok(Self { schema, entries: names })
    }

    pub(crate) fn require_schema(&self, descriptor: &WireMessageDescriptor) -> Result<(), WirePreflightError> {
        if !Arc::ptr_eq(&self.schema, &descriptor.schema) {
            return Err(WirePreflightError::MapProfileSchema);
        }
        Ok(())
    }

    pub(crate) fn permits(&self, name: &str) -> bool {
        self.entries.contains(name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WireMessageMapProfileError {
    #[error("message map profile does not match a bounded exact string-key/message-value schema")]
    Invalid,
}
