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
            Tier1TupleResourceShape::AgentCardView => {
                Tier1ResourceTuple::new("agent_card", format!("{publisher_account}/{agent_id}"), "view")
            }
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Tier1DeriveError {
    #[error("task id missing from params")]
    MissingTaskId,
    #[error("no tier-1 resource tuple for method")]
    UnknownMethod,
}

fn task_id_from_params(params: &Value) -> Option<String> {
    params
        .get("id")
        .or_else(|| params.get("task_id"))
        .or_else(|| params.get("taskId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests;
