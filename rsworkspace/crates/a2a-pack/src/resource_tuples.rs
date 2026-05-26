use std::fmt;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1ResourceType(String);

impl Tier1ResourceType {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1ResourceId(String);

impl Tier1ResourceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1Permission(String);

impl Tier1Permission {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1ResourceTuple {
    pub resource_type: Tier1ResourceType,
    pub resource_id: Tier1ResourceId,
    pub permission: Tier1Permission,
}

impl Tier1ResourceTuple {
    pub fn new(
        resource_type: impl Into<String>,
        resource_id: impl Into<String>,
        permission: impl Into<String>,
    ) -> Self {
        Self {
            resource_type: Tier1ResourceType(resource_type.into()),
            resource_id: Tier1ResourceId(resource_id.into()),
            permission: Tier1Permission(permission.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1A2aMethodSlug(String);

impl Tier1A2aMethodSlug {
    pub fn new(slug: impl Into<String>) -> Self {
        Self(slug.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier1TupleResourceShape {
    AgentCardView,
    AgentInvoke,
    AgentDiscover,
    TaskRead,
    TaskCancel,
    TaskConfigurePush,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1ResourceTupleRow {
    pub method: Tier1A2aMethodSlug,
    pub shape: Tier1TupleResourceShape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1ResourceTupleTable {
    rows: Vec<Tier1ResourceTupleRow>,
}

impl Tier1ResourceTupleTable {
    pub fn bundled() -> Self {
        Self {
            rows: vec![
                row("agent/card", Tier1TupleResourceShape::AgentCardView),
                row("message/send", Tier1TupleResourceShape::AgentInvoke),
                row("message/stream", Tier1TupleResourceShape::AgentInvoke),
                row("tasks/list", Tier1TupleResourceShape::AgentDiscover),
                row("tasks/get", Tier1TupleResourceShape::TaskRead),
                row("tasks/resubscribe", Tier1TupleResourceShape::TaskRead),
                row("tasks/cancel", Tier1TupleResourceShape::TaskCancel),
                row(
                    "tasks/pushNotificationConfig/set",
                    Tier1TupleResourceShape::TaskConfigurePush,
                ),
                row(
                    "tasks/pushNotificationConfig/get",
                    Tier1TupleResourceShape::TaskConfigurePush,
                ),
                row(
                    "tasks/pushNotificationConfig/list",
                    Tier1TupleResourceShape::TaskConfigurePush,
                ),
                row(
                    "tasks/pushNotificationConfig/delete",
                    Tier1TupleResourceShape::TaskConfigurePush,
                ),
            ],
        }
    }

    pub fn rows(&self) -> &[Tier1ResourceTupleRow] {
        &self.rows
    }

    pub fn derive(
        &self,
        method: &Tier1A2aMethodSlug,
        agent_id: &str,
        publisher_account: &str,
        params: &Value,
    ) -> Result<Tier1ResourceTuple, Tier1DeriveError> {
        let shape = self
            .rows
            .iter()
            .find(|row| row.method == *method)
            .map(|row| row.shape)
            .ok_or(Tier1DeriveError::UnknownMethod)?;

        Ok(match shape {
            Tier1TupleResourceShape::AgentCardView => Tier1ResourceTuple::new(
                "agent_card",
                format!("{publisher_account}/{agent_id}"),
                "view",
            ),
            Tier1TupleResourceShape::AgentInvoke => Tier1ResourceTuple::new("agent", agent_id, "invoke"),
            Tier1TupleResourceShape::AgentDiscover => Tier1ResourceTuple::new("agent", agent_id, "discover"),
            Tier1TupleResourceShape::TaskRead => {
                let task_id = task_id_from_params(params).ok_or(Tier1DeriveError::MissingTaskId)?;
                Tier1ResourceTuple::new("task", format!("{agent_id}:{task_id}"), "read")
            }
            Tier1TupleResourceShape::TaskCancel => {
                let task_id = task_id_from_params(params).ok_or(Tier1DeriveError::MissingTaskId)?;
                Tier1ResourceTuple::new("task", format!("{agent_id}:{task_id}"), "cancel")
            }
            Tier1TupleResourceShape::TaskConfigurePush => {
                let task_id = task_id_from_params(params).ok_or(Tier1DeriveError::MissingTaskId)?;
                Tier1ResourceTuple::new("task", format!("{agent_id}:{task_id}"), "configure_push")
            }
        })
    }
}

fn row(method: &str, shape: Tier1TupleResourceShape) -> Tier1ResourceTupleRow {
    Tier1ResourceTupleRow {
        method: Tier1A2aMethodSlug::new(method),
        shape,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tier1DeriveError {
    MissingTaskId,
    UnknownMethod,
}

impl fmt::Display for Tier1DeriveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTaskId => write!(f, "task id missing from params"),
            Self::UnknownMethod => write!(f, "no tier-1 resource tuple for method"),
        }
    }
}

impl std::error::Error for Tier1DeriveError {}

fn task_id_from_params(params: &Value) -> Option<String> {
    params
        .get("id")
        .or_else(|| params.get("task_id"))
        .or_else(|| params.get("taskId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Tier1ResourceTupleTable {
        Tier1ResourceTupleTable::bundled()
    }

    #[test]
    fn derive_message_send_uses_agent_invoke() {
        let tuple = table()
            .derive(
                &Tier1A2aMethodSlug::new("message/send"),
                "planner",
                "pub",
                &serde_json::json!({}),
            )
            .unwrap();
        assert_eq!(tuple.resource_type.as_str(), "agent");
        assert_eq!(tuple.resource_id.as_str(), "planner");
        assert_eq!(tuple.permission.as_str(), "invoke");
    }

    #[test]
    fn derive_tasks_list_uses_discover() {
        let tuple = table()
            .derive(
                &Tier1A2aMethodSlug::new("tasks/list"),
                "planner",
                "pub",
                &serde_json::json!({}),
            )
            .unwrap();
        assert_eq!(tuple.permission.as_str(), "discover");
    }

    #[test]
    fn derive_agent_card_uses_agent_card_resource() {
        let tuple = table()
            .derive(
                &Tier1A2aMethodSlug::new("agent/card"),
                "planner",
                "publisher",
                &serde_json::json!({}),
            )
            .unwrap();
        assert_eq!(tuple.resource_type.as_str(), "agent_card");
        assert_eq!(tuple.resource_id.as_str(), "publisher/planner");
    }

    #[test]
    fn derive_tasks_get_requires_task_id() {
        let err = table()
            .derive(
                &Tier1A2aMethodSlug::new("tasks/get"),
                "planner",
                "pub",
                &serde_json::json!({}),
            )
            .unwrap_err();
        assert_eq!(err, Tier1DeriveError::MissingTaskId);
    }
}
