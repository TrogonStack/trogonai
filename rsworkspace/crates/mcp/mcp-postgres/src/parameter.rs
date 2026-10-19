use crate::Error;
use rmcp::schemars;
use serde::Deserialize;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ParameterInput {
    Boolean(bool),
    Integer(i64),
    Text(String),
}
#[derive(Debug, Clone)]
pub struct Parameter(ParameterValue);
#[derive(Debug, Clone)]
pub(crate) enum ParameterValue {
    Boolean(bool),
    Integer(i64),
    Text(String),
}
impl TryFrom<ParameterInput> for Parameter {
    type Error = Error;
    fn try_from(input: ParameterInput) -> Result<Self, Error> {
        Ok(Self(match input {
            ParameterInput::Boolean(v) => ParameterValue::Boolean(v),
            ParameterInput::Integer(v) => ParameterValue::Integer(v),
            ParameterInput::Text(v) if v.len() <= 16_384 && !v.contains('\0') => ParameterValue::Text(v),
            ParameterInput::Text(_) => return Err(Error::Limit),
        }))
    }
}
impl Parameter {
    pub(crate) fn value(&self) -> &ParameterValue {
        &self.0
    }
}
