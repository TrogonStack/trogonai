use buffa::{EnumValue, Enumeration};
use trogonai_catalog_proto::ModelModality;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub model_id: String,
    pub provider: String,
    pub context_window: u64,
    pub max_output: u32,
    pub modality: ModelModality,
}

impl CatalogEntry {
    pub fn is_text_output(&self) -> bool {
        matches!(
            self.modality,
            ModelModality::TEXT | ModelModality::MODEL_MODALITY_UNSPECIFIED
        )
    }
}

pub(crate) fn modality_from_proto(value: EnumValue<ModelModality>) -> ModelModality {
    match value {
        EnumValue::Known(v) => v,
        EnumValue::Unknown(n) => ModelModality::from_i32(n).unwrap_or(ModelModality::MODEL_MODALITY_UNSPECIFIED),
    }
}
