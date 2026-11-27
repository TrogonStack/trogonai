use serde_json::Value;
use std::path::Path;

use crate::acl_outcome::AclOutcome;
use crate::constants::RELEASED;

/// What a release of the session's grants came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclRevoke {
    /// `fence-helper acl revoke` exited 0, printed outcomes, and every one shows
    /// its grant released: the gate for [`WindowsFence::acl_restore`](crate::WindowsFence::acl_restore).
    pub complete: bool,
    pub outcomes: Option<Vec<AclOutcome>>,
    /// Paths whose outcome does not show the grant released.
    pub pending: Vec<String>,
    pub detail: Option<String>,
}

fn released(outcome: &AclOutcome, gone: &dyn Fn(&str) -> bool) -> bool {
    if !RELEASED.contains(&outcome.status.as_str()) {
        return false;
    }
    outcome.status != "missing" || gone(&outcome.path)
}

pub(crate) fn path_is_gone(path: &str) -> bool {
    matches!(
        std::fs::symlink_metadata(Path::new(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

pub(crate) fn judge_revoke(exited_zero: bool, stdout: &str, stderr: &str, gone: &dyn Fn(&str) -> bool) -> AclRevoke {
    let outcomes: Option<Vec<AclOutcome>> = serde_json::from_str(stdout).ok();
    let pending: Vec<String> = outcomes
        .iter()
        .flatten()
        .filter(|outcome| !released(outcome, gone))
        .map(|outcome| outcome.path.clone())
        .collect();
    AclRevoke {
        complete: exited_zero && outcomes.is_some() && pending.is_empty(),
        outcomes,
        pending,
        detail: Some(stderr.trim().to_string()).filter(|text| !text.is_empty()),
    }
}

pub(crate) fn flatten_restore(stdout: &str) -> Option<Vec<AclOutcome>> {
    match serde_json::from_str::<Value>(stdout).ok()? {
        Value::Array(items) => serde_json::from_value(Value::Array(items)).ok(),
        Value::Object(map) => {
            let mut out = Vec::new();
            for field in ["paths", "parents"] {
                if let Some(items) = map.get(field) {
                    out.extend(serde_json::from_value::<Vec<AclOutcome>>(items.clone()).ok()?);
                }
            }
            Some(out)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
