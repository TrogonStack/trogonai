use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaSource {
    ToolsListSniff,
    ExplicitRegistration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedSchema {
    pub schema: serde_json::Value,
    pub fetched_at: SystemTime,
    pub source: SchemaSource,
}
