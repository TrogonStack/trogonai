use trogonai_session_contracts::ContractValidationError;

#[derive(Debug, thiserror::Error)]
pub enum ProjectionError {
    #[error("contract validation failed: {0}")]
    ContractValidation(#[from] ContractValidationError),

    #[error("context twin payload exceeds limit: {actual} bytes (limit {limit})")]
    ContextTwinTooLarge { actual: usize, limit: usize },

    #[error("failed to decode context twin: {0}")]
    Decode(String),

    #[error("critical projection blocks do not fit within token budget for model {model_id}")]
    CriticalBlocksDoNotFit { model_id: String },

    #[error("session state missing required field: {0}")]
    MissingField(&'static str),

    #[error("failed to load context twin: {0}")]
    ContextTwinLoad(String),

    #[error("failed to store context twin: {0}")]
    ContextTwinStore(String),

    #[error("failed to provision context twin store: {0}")]
    Provision(String),

    #[error("session kernel error: {0}")]
    Kernel(#[from] trogonai_session_kernel::SessionKernelError),
}
