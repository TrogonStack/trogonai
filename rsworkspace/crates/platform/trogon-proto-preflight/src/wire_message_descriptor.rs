use std::sync::Arc;

use crate::schema_data::SchemaData;

#[derive(Clone, Debug)]
pub struct WireMessageDescriptor {
    pub(crate) name: String,
    pub(crate) schema: Arc<SchemaData>,
}
