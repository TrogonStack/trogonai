use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default)]
pub(crate) struct SchemaData {
    pub messages: BTreeMap<String, MessageShape>,
    pub enums: BTreeMap<String, BTreeSet<i32>>,
}

#[derive(Debug)]
pub(crate) struct MessageShape {
    pub fields: BTreeMap<u32, FieldShape>,
    pub map_entry: bool,
    pub has_oneof: bool,
}

#[derive(Debug)]
pub(crate) struct FieldShape {
    pub kind: String,
    pub type_name: Option<String>,
    pub repeated: bool,
    pub required: bool,
    pub oneof: Option<u32>,
}
