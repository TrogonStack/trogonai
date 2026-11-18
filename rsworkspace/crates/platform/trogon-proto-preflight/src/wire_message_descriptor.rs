use std::sync::Arc;

use crate::wire_schema_pool::SchemaData;

#[derive(Clone, Debug)]
pub struct WireMessageDescriptor {
    pub(crate) name: String,
    pub(crate) schema: Arc<SchemaData>,
}
