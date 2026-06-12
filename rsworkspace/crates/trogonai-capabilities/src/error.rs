use trogon_registry::RegistryError;

#[derive(Debug, thiserror::Error)]
pub enum CapabilityError {
    #[error("model not found in registry: {model_id}")]
    ModelNotFound { model_id: String },

    #[error("capability schema missing for model {model_id} (runner {runner_id})")]
    SchemaMissing { model_id: String, runner_id: String },

    #[error("failed to decode capability schema for model {model_id}: {detail}")]
    SchemaDecode { model_id: String, detail: String },

    #[error("registry error: {0}")]
    Registry(#[from] RegistryError),

    #[error("session has no configured model")]
    SessionModelMissing,

    #[error("invalid certification matrix entry: {detail}")]
    InvalidCertification { detail: String },
}
