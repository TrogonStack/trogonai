use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditPolicyDecision {
    Allow,
    Deny,
    Refuse,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditCallerVia(String);

impl AuditCallerVia {
    pub fn new(via: impl Into<String>) -> Self {
        Self(via.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditTenantId(String);

impl AuditTenantId {
    pub fn new(tenant: impl Into<String>) -> Self {
        Self(tenant.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditTaskLifecycleId(String);

impl AuditTaskLifecycleId {
    pub fn new(task_id: impl Into<String>) -> Self {
        Self(task_id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditTaskStateLabel(String);

impl AuditTaskStateLabel {
    pub fn new(state: impl Into<String>) -> Self {
        Self(state.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AuditEnvelopeExtensions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<AuditTenantId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_via: Option<AuditCallerVia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<AuditTaskLifecycleId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_state_before: Option<AuditTaskStateLabel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_state_after: Option<AuditTaskStateLabel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_decision: Option<AuditPolicyDecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_us: Option<u64>,
}

impl AuditEnvelopeExtensions {
    pub fn apply_to_json(&self, envelope: &mut serde_json::Value) {
        let Some(map) = envelope.as_object_mut() else {
            return;
        };
        if let Some(tenant) = &self.tenant {
            map.insert("tenant".into(), serde_json::Value::String(tenant.as_str().to_owned()));
        }
        if let Some(via) = &self.caller_via {
            map.insert(
                "caller_via".into(),
                serde_json::Value::String(via.as_str().to_owned()),
            );
        }
        if let Some(task_id) = &self.task_id {
            map.insert(
                "task_id".into(),
                serde_json::Value::String(task_id.as_str().to_owned()),
            );
        }
        if let Some(before) = &self.task_state_before {
            map.insert(
                "task_state_before".into(),
                serde_json::Value::String(before.as_str().to_owned()),
            );
        }
        if let Some(after) = &self.task_state_after {
            map.insert(
                "task_state_after".into(),
                serde_json::Value::String(after.as_str().to_owned()),
            );
        }
        if let Some(decision) = &self.policy_decision {
            map.insert(
                "policy_decision".into(),
                serde_json::to_value(decision).expect("policy decision serializes"),
            );
        }
        if let Some(latency_us) = self.latency_us {
            map.insert("latency_us".into(), serde_json::Value::from(latency_us));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_apply_to_envelope_json() {
        let mut envelope = serde_json::json!({
            "agent_id": "bot",
            "method": "message/send",
            "outcome": "ok"
        });
        let ext = AuditEnvelopeExtensions {
            tenant: Some(AuditTenantId::new("acme")),
            policy_decision: Some(AuditPolicyDecision::Allow),
            latency_us: Some(1_820),
            ..AuditEnvelopeExtensions::default()
        };
        ext.apply_to_json(&mut envelope);
        assert_eq!(envelope["tenant"], "acme");
        assert_eq!(envelope["policy_decision"], "allow");
        assert_eq!(envelope["latency_us"], 1_820);
    }
}
