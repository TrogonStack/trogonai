use crate::{WireMessageDescriptor, WirePreflightError, wire_schema_pool::SchemaData};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Debug)]
pub struct WireStringMapProfile {
    schema: Arc<SchemaData>,
    entries: BTreeSet<String>,
}

impl WireStringMapProfile {
    pub(crate) fn new(schema: Arc<SchemaData>, names: &[&str]) -> Result<Self, WireStringMapProfileError> {
        if names.is_empty() || names.len() > 32 {
            return Err(WireStringMapProfileError::EntryCount);
        }
        let mut entries = BTreeSet::new();
        for name in names {
            if name.is_empty() || name.len() > 1024 {
                return Err(WireStringMapProfileError::NameLength);
            }
            if !entries.insert((*name).to_owned()) {
                return Err(WireStringMapProfileError::DuplicateEntry);
            }
            let shape = schema
                .messages
                .get(*name)
                .ok_or(WireStringMapProfileError::UnknownEntry)?;
            if !shape.map_entry
                || shape.has_oneof
                || shape.fields.len() != 2
                || ![1, 2].into_iter().all(|number| {
                    shape.fields.get(&number).is_some_and(|field| {
                        field.kind == "TYPE_STRING"
                            && !field.repeated
                            && !field.required
                            && field.oneof.is_none()
                            && field.type_name.is_none()
                    })
                })
            {
                return Err(WireStringMapProfileError::EntryShape);
            }
        }
        Ok(Self { schema, entries })
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
pub enum WireStringMapProfileError {
    #[error("string map profile requires between one and 32 entries")]
    EntryCount,
    #[error("string map profile entry name is outside its byte bound")]
    NameLength,
    #[error("string map profile repeats an entry")]
    DuplicateEntry,
    #[error("string map profile entry is unknown")]
    UnknownEntry,
    #[error("string map profile entry is not an exact singular string key/value map")]
    EntryShape,
}
