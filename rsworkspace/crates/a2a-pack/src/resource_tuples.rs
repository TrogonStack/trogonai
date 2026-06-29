use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Tier1ValueObjectError {
    #[error("tier-1 resource type must not be empty")]
    EmptyResourceType,
    #[error("tier-1 resource id must not be empty")]
    EmptyResourceId,
    #[error("tier-1 permission must not be empty")]
    EmptyPermission,
    #[error("tier-1 A2A method slug must not be empty")]
    EmptyMethodSlug,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1ResourceType(String);

impl Tier1ResourceType {
    /// Validated constructor — rejects empty/whitespace-only resource
    /// type names so an invalid Tier-1 tuple is unrepresentable at
    /// the type level. SpiceDb tuples that hit the wire with an empty
    /// `resource_type` produce silent denials, so failing closed at
    /// construction surfaces the bug at config-load time.
    pub fn new(value: impl Into<String>) -> Result<Self, Tier1ValueObjectError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(Tier1ValueObjectError::EmptyResourceType);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1ResourceId(String);

impl Tier1ResourceId {
    pub fn new(value: impl Into<String>) -> Result<Self, Tier1ValueObjectError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(Tier1ValueObjectError::EmptyResourceId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1Permission(String);

impl Tier1Permission {
    pub fn new(value: impl Into<String>) -> Result<Self, Tier1ValueObjectError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(Tier1ValueObjectError::EmptyPermission);
        }
        Ok(Self(value))
    }

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
    /// Build a tuple from already-validated value objects. There is
    /// no `(String, String, String)` convenience constructor on
    /// purpose — every callsite must pass the typed inputs so an
    /// invalid tuple can't reach storage or the SpiceDb client.
    pub fn new(resource_type: Tier1ResourceType, resource_id: Tier1ResourceId, permission: Tier1Permission) -> Self {
        Self {
            resource_type,
            resource_id,
            permission,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1A2aMethodSlug(String);

impl Tier1A2aMethodSlug {
    pub fn new(slug: impl Into<String>) -> Result<Self, Tier1ValueObjectError> {
        let slug = slug.into();
        if slug.trim().is_empty() {
            return Err(Tier1ValueObjectError::EmptyMethodSlug);
        }
        Ok(Self(slug))
    }

    /// Crate-private bypass for tables of statically-known slugs.
    /// Keeps the bundled table free of an `expect()` panic path
    /// while preserving the validating public constructor for
    /// untrusted callers.
    pub(crate) fn new_unchecked(slug: &'static str) -> Self {
        Self(slug.to_owned())
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
        let rows = [
            ("agent/card", Tier1TupleResourceShape::AgentCardView),
            ("message/send", Tier1TupleResourceShape::AgentInvoke),
            ("message/stream", Tier1TupleResourceShape::AgentInvoke),
            ("tasks/list", Tier1TupleResourceShape::AgentDiscover),
            ("tasks/get", Tier1TupleResourceShape::TaskRead),
            ("tasks/resubscribe", Tier1TupleResourceShape::TaskRead),
            ("tasks/cancel", Tier1TupleResourceShape::TaskCancel),
            (
                "tasks/pushNotificationConfig/set",
                Tier1TupleResourceShape::TaskConfigurePush,
            ),
            (
                "tasks/pushNotificationConfig/get",
                Tier1TupleResourceShape::TaskConfigurePush,
            ),
            (
                "tasks/pushNotificationConfig/list",
                Tier1TupleResourceShape::TaskConfigurePush,
            ),
            (
                "tasks/pushNotificationConfig/delete",
                Tier1TupleResourceShape::TaskConfigurePush,
            ),
        ];

        Self {
            rows: rows
                .iter()
                .map(|(method, shape)| Tier1ResourceTupleRow {
                    method: Tier1A2aMethodSlug::new_unchecked(method),
                    shape: *shape,
                })
                .collect(),
        }
    }

    pub fn rows(&self) -> &[Tier1ResourceTupleRow] {
        &self.rows
    }

    pub fn shape_for(&self, method: &Tier1A2aMethodSlug) -> Option<Tier1TupleResourceShape> {
        self.rows.iter().find(|row| row.method == *method).map(|row| row.shape)
    }

    /// Derive a typed Tier-1 tuple from already-extracted inputs.
    /// Wire-shape parsing (which JSON-RPC param key holds the task
    /// id, etc.) lives in [`Tier1DeriveInputs::from_jsonrpc_params`]
    /// so the domain layer only ever sees validated, key-agnostic
    /// inputs.
    pub fn derive(&self, inputs: &Tier1DeriveInputs) -> Result<Tier1ResourceTuple, Tier1DeriveError> {
        let shape = self.shape_for(&inputs.method).ok_or(Tier1DeriveError::UnknownMethod)?;

        let tuple = match shape {
            Tier1TupleResourceShape::AgentCardView => Tier1ResourceTuple::new(
                Tier1ResourceType::new("agent_card")?,
                Tier1ResourceId::new(format!("{}/{}", inputs.publisher_account, inputs.agent_id))?,
                Tier1Permission::new("view")?,
            ),
            Tier1TupleResourceShape::AgentInvoke => Tier1ResourceTuple::new(
                Tier1ResourceType::new("agent")?,
                Tier1ResourceId::new(&inputs.agent_id)?,
                Tier1Permission::new("invoke")?,
            ),
            Tier1TupleResourceShape::AgentDiscover => Tier1ResourceTuple::new(
                Tier1ResourceType::new("agent")?,
                Tier1ResourceId::new(&inputs.agent_id)?,
                Tier1Permission::new("discover")?,
            ),
            Tier1TupleResourceShape::TaskRead => {
                let task_id = inputs.task_id.as_deref().ok_or(Tier1DeriveError::MissingTaskId)?;
                Tier1ResourceTuple::new(
                    Tier1ResourceType::new("task")?,
                    Tier1ResourceId::new(format!("{}:{}", inputs.agent_id, task_id))?,
                    Tier1Permission::new("read")?,
                )
            }
            Tier1TupleResourceShape::TaskCancel => {
                let task_id = inputs.task_id.as_deref().ok_or(Tier1DeriveError::MissingTaskId)?;
                Tier1ResourceTuple::new(
                    Tier1ResourceType::new("task")?,
                    Tier1ResourceId::new(format!("{}:{}", inputs.agent_id, task_id))?,
                    Tier1Permission::new("cancel")?,
                )
            }
            Tier1TupleResourceShape::TaskConfigurePush => {
                let task_id = inputs.task_id.as_deref().ok_or(Tier1DeriveError::MissingTaskId)?;
                Tier1ResourceTuple::new(
                    Tier1ResourceType::new("task")?,
                    Tier1ResourceId::new(format!("{}:{}", inputs.agent_id, task_id))?,
                    Tier1Permission::new("configure_push")?,
                )
            }
        };

        Ok(tuple)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Tier1DeriveError {
    #[error("task id missing from params")]
    MissingTaskId,
    #[error("no tier-1 resource tuple for method")]
    UnknownMethod,
    #[error(transparent)]
    ValueObject(#[from] Tier1ValueObjectError),
}

/// Typed boundary inputs for [`Tier1ResourceTupleTable::derive`].
///
/// Building one of these is the single place where the wire JSON-RPC
/// param shape collapses into the domain view — once
/// `from_jsonrpc_params` returns, no downstream layer needs to know
/// which key in `params` carries the task id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tier1DeriveInputs {
    pub method: Tier1A2aMethodSlug,
    pub agent_id: String,
    pub publisher_account: String,
    pub task_id: Option<String>,
}

impl Tier1DeriveInputs {
    /// Parse the task id from a JSON-RPC `params` object using the
    /// per-shape precedence the A2A spec mandates:
    ///
    /// - `tasks/pushNotificationConfig/{get,delete}` params carry
    ///   `taskId` for the task and `id` for the push config — reading
    ///   `id` first would target the wrong resource at SpiceDb.
    /// - The plain task methods (`tasks/get`, `tasks/cancel`,
    ///   `tasks/resubscribe`) use the top-level `id` for the task.
    pub fn from_jsonrpc_params(
        method: Tier1A2aMethodSlug,
        agent_id: impl Into<String>,
        publisher_account: impl Into<String>,
        params: &Value,
    ) -> Self {
        let task_id = task_id_from_params(method.as_str(), params);
        Self {
            method,
            agent_id: agent_id.into(),
            publisher_account: publisher_account.into(),
            task_id,
        }
    }
}

fn task_id_from_params(method: &str, params: &Value) -> Option<String> {
    // Push-notification-config methods bind `taskId` to the task and
    // `id` to the push config row. Other task methods use `id` (or
    // `task_id` snake_case) for the task itself.
    let order: &[&str] = if is_push_notification_config_method(method) {
        &["taskId", "task_id", "id"]
    } else {
        &["id", "taskId", "task_id"]
    };
    for key in order {
        if let Some(value) = params.get(*key).and_then(Value::as_str) {
            // Skip empty/whitespace-only strings so they fall through
            // to `MissingTaskId` rather than producing SpiceDb tuples
            // like `agent:` that pass `Tier1ResourceId` validation but
            // never match a real task.
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    None
}

fn is_push_notification_config_method(method: &str) -> bool {
    method.starts_with("tasks/pushNotificationConfig/")
}

#[cfg(test)]
mod tests;
