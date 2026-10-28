use serde_json::Value;

use super::constants::{NATS_HEADER_API_APP_ID, NATS_HEADER_ENTERPRISE_ID, NATS_HEADER_IS_ENTERPRISE_INSTALL};
use super::header_text::{DeliveryHeaders, InvalidHeaderTextError};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PayloadMetadata<'a> {
    enterprise_id: Option<&'a str>,
    is_enterprise_install: bool,
    api_app_id: Option<&'a str>,
}

impl<'a> PayloadMetadata<'a> {
    pub fn event(payload: &'a Value) -> Self {
        let authorization = payload
            .get("authorizations")
            .and_then(Value::as_array)
            .and_then(|authorizations| authorizations.first());
        let (enterprise_id, is_enterprise_install) = match authorization {
            Some(authorization) => (
                text(authorization, "enterprise_id"),
                authorization.get("is_enterprise_install") == Some(&Value::Bool(true)),
            ),
            None => (text(payload, "enterprise_id"), install_flag(payload)),
        };
        Self {
            enterprise_id,
            is_enterprise_install,
            api_app_id: text(payload, "api_app_id"),
        }
    }

    pub fn interaction(payload: &'a Value) -> Self {
        let enterprise = payload.get("enterprise").filter(|value| !value.is_null());
        let team = payload.get("team").filter(|value| !value.is_null());
        let enterprise_id = match (enterprise, team) {
            (Some(enterprise), _) => text(enterprise, "id"),
            (None, Some(team)) => text(team, "enterprise_id"),
            (None, None) => None,
        };
        Self {
            enterprise_id,
            is_enterprise_install: install_flag(payload),
            api_app_id: text(payload, "api_app_id"),
        }
    }

    pub fn command_value(payload: &'a Value) -> Self {
        Self {
            enterprise_id: text(payload, "enterprise_id"),
            is_enterprise_install: install_flag(payload),
            api_app_id: text(payload, "api_app_id"),
        }
    }

    pub fn command_fields(fields: &'a [(String, String)]) -> Self {
        let field = |name: &str| {
            fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
                .filter(|value| !value.is_empty())
        };
        Self {
            enterprise_id: field("enterprise_id"),
            is_enterprise_install: field("is_enterprise_install") == Some("true"),
            api_app_id: field("api_app_id"),
        }
    }

    pub fn apply(&self, headers: DeliveryHeaders) -> Result<DeliveryHeaders, InvalidHeaderTextError> {
        let install = if self.is_enterprise_install { "true" } else { "false" };
        headers
            .with_optional(NATS_HEADER_ENTERPRISE_ID, self.enterprise_id)?
            .with_static(NATS_HEADER_IS_ENTERPRISE_INSTALL, install)
            .with_optional(NATS_HEADER_API_APP_ID, self.api_app_id)
    }
}

fn text<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(Value::as_str).filter(|text| !text.is_empty())
}

fn install_flag(payload: &Value) -> bool {
    match payload.get("is_enterprise_install") {
        Some(Value::String(flag)) => flag == "true",
        Some(Value::Bool(flag)) => *flag,
        _ => false,
    }
}

#[cfg(test)]
mod tests;
