//! Reads the network policy fields of an owned Daytona sandbox into shapes that carry no values, so a hosted report
//! can say whether the policy reached the provider without echoing it.

use serde_json::Value;

/// Most bytes of a sandbox record the observer reads.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

/// The shape of one JSON field, with its value dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Missing,
    Null,
    BooleanTrue,
    BooleanFalse,
    StringEmpty,
    StringNonempty,
    ArrayEmpty,
    ArrayStrings,
    ArrayMixed,
    Number,
    Object,
}

impl Field {
    pub fn of(value: Option<&Value>) -> Self {
        match value {
            None => Self::Missing,
            Some(Value::Null) => Self::Null,
            Some(Value::Bool(true)) => Self::BooleanTrue,
            Some(Value::Bool(false)) => Self::BooleanFalse,
            Some(Value::String(text)) if text.is_empty() => Self::StringEmpty,
            Some(Value::String(_)) => Self::StringNonempty,
            Some(Value::Array(items)) if items.is_empty() => Self::ArrayEmpty,
            Some(Value::Array(items)) if items.iter().all(Value::is_string) => Self::ArrayStrings,
            Some(Value::Array(_)) => Self::ArrayMixed,
            Some(Value::Number(_)) => Self::Number,
            Some(Value::Object(_)) => Self::Object,
        }
    }
}

/// What a lookup of one owned sandbox found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    OwnedNativePresent {
        network_block_all: Field,
        network_allow_list: Field,
    },
    ReconciliationRequired,
    RequestFailed,
    NativeAbsent,
    UnexpectedHttpStatus,
    BodyTooLarge,
    JsonMalformed,
    IdentityMismatch,
    OrganizationMismatch,
    LeaseLabelMismatch,
}

/// The identity a record must carry to count as owned by the lease under test.
pub struct Owner<'a> {
    pub sandbox_id: &'a str,
    pub lease_id: &'a str,
    pub organization_id: &'a str,
}

/// Reads one sandbox response, accepting it only when its id, organization and lease label all match `owner`.
pub fn parse_owned(status: u16, body: &[u8], owner: &Owner<'_>) -> Observation {
    match status {
        200 => {}
        404 => return Observation::NativeAbsent,
        _ => return Observation::UnexpectedHttpStatus,
    }
    if body.len() > MAX_BODY_BYTES {
        return Observation::BodyTooLarge;
    }
    let Ok(record) = serde_json::from_slice::<Value>(body) else {
        return Observation::JsonMalformed;
    };
    if record.get("id").and_then(Value::as_str) != Some(owner.sandbox_id) {
        return Observation::IdentityMismatch;
    }
    if record.get("organizationId").and_then(Value::as_str) != Some(owner.organization_id) {
        return Observation::OrganizationMismatch;
    }
    let label = record
        .get("labels")
        .and_then(|labels| labels.get("trogon.lease_id"))
        .and_then(Value::as_str);
    if label != Some(owner.lease_id) {
        return Observation::LeaseLabelMismatch;
    }
    Observation::OwnedNativePresent {
        network_block_all: Field::of(record.get("networkBlockAll")),
        network_allow_list: Field::of(record.get("networkAllowList")),
    }
}

/// What a run of the egress check observed about the two leases it created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub allow_all_control: Observation,
    pub deny_all_restricted: Observation,
    pub unexpected_grant_count: usize,
}

impl Report {
    /// Whether both leases were seen, the control unrestricted, the restricted lease blocked, and no other lease
    /// appeared.
    pub fn complete(&self) -> bool {
        let control = matches!(
            self.allow_all_control,
            Observation::OwnedNativePresent {
                network_block_all: Field::Missing | Field::Null | Field::BooleanFalse,
                ..
            }
        );
        let restricted = matches!(
            self.deny_all_restricted,
            Observation::OwnedNativePresent {
                network_block_all: Field::BooleanTrue,
                ..
            }
        );
        control && restricted && self.unexpected_grant_count == 0
    }
}

#[cfg(test)]
mod tests;
