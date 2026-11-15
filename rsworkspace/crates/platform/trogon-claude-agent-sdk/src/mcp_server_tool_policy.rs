use crate::JsonObject;
use serde::Serialize;
use serde_json::Value;

/// TypeScript `McpServerToolPolicy`: a per-tool permission policy on a server config.
#[derive(Clone, Debug)]
pub struct McpServerToolPolicy {
    pub name: String,
    pub permission_policy: Option<McpToolPermissionPolicy>,
    pub org_max_permission: Option<McpToolOrgMaxPermission>,
    /// Keys sent after the typed fields, verbatim.
    pub extra: JsonObject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpToolPermissionPolicy {
    AlwaysAllow,
    AlwaysAsk,
    AlwaysDeny,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpToolOrgMaxPermission {
    Allow,
    Ask,
    Blocked,
}

impl McpServerToolPolicy {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            permission_policy: None,
            org_max_permission: None,
            extra: JsonObject::new(),
        }
    }

    pub(crate) fn to_wire_json(&self) -> Result<Value, serde_json::Error> {
        let mut object = JsonObject::new();
        object.insert("name".into(), self.name.clone().into());
        if let Some(policy) = self.permission_policy {
            object.insert("permission_policy".into(), serde_json::to_value(policy)?);
        }
        if let Some(maximum) = self.org_max_permission {
            object.insert("org_max_permission".into(), serde_json::to_value(maximum)?);
        }
        for (key, value) in &self.extra {
            if !object.contains_key(key) {
                object.insert(key.clone(), value.clone());
            }
        }
        Ok(Value::Object(object))
    }
}
