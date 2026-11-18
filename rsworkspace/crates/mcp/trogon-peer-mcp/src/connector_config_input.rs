use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorConfigInput {
    pub installation_id: String,
    pub connector_id: String,
    pub collaboration_ref: String,
    pub display_name: String,
}
