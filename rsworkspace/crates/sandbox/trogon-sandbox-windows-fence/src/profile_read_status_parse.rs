use crate::constants::RECORD_VERSION;
use crate::profile_exclusions::ProfileExclusions;
use crate::profile_grant_counts::ProfileGrantCounts;
use crate::profile_grant_item::items;
use crate::profile_grant_path::ProfileGrantPath;
use crate::profile_grant_path_kind::ProfileGrantPathKind;
use crate::profile_read_state::ProfileReadState;
use crate::profile_read_status::ProfileReadStatus;
use crate::profile_read_status_wire::ProfileReadStatusWire;
use crate::profile_read_var::ProfileReadVar;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

fn bad(field: &str, why: &str) -> WindowsSandboxError {
    WindowsSandboxError::for_subcommand(
        Code::FenceHelperBadJson,
        format!("fence-helper profile status: `{field}` {why}"),
        "profile",
    )
}

fn text_list(field: &str, value: Option<&serde_json::Value>) -> Result<Vec<String>, WindowsSandboxError> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(rows)) => rows
            .iter()
            .map(|row| {
                row.as_str()
                    .map(String::from)
                    .ok_or_else(|| bad(field, "has an entry that is not text"))
            })
            .collect(),
        Some(_) => Err(bad(field, "is not a list")),
    }
}

fn grant_path(row: &serde_json::Value) -> Result<ProfileGrantPath, WindowsSandboxError> {
    let path = row
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| bad("paths", "has a row without a text `path`"))?
        .to_string();
    let kind = match row.get("kind").and_then(|v| v.as_str()) {
        Some("container") => ProfileGrantPathKind::Container,
        Some("granted") => ProfileGrantPathKind::Granted,
        Some("file") => ProfileGrantPathKind::File,
        Some("denied") => ProfileGrantPathKind::Denied,
        _ => return Err(bad("paths", "has a row of an unknown `kind`")),
    };
    Ok(ProfileGrantPath { path, kind })
}

fn string_list(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    value?
        .as_array()?
        .iter()
        .map(|item| item.as_str().filter(|text| !text.is_empty()).map(String::from))
        .collect()
}

fn status_problem(raw: &ProfileReadStatusWire) -> Option<String> {
    if raw.record_version.as_ref().and_then(|v| v.as_u64()) != Some(RECORD_VERSION) {
        let seen = raw
            .record_version
            .as_ref()
            .map_or_else(|| "undefined".to_string(), |v| v.to_string());
        return Some(format!(
            "fence-helper reports record format {seen}; this library reads format {RECORD_VERSION}"
        ));
    }
    match string_list(raw.exclusions.as_ref()) {
        Some(list) if !list.is_empty() => {}
        _ => return Some("the recorded grant carries no list of excluded paths".to_string()),
    }
    if string_list(raw.ungranted_only.as_ref()).is_none() {
        return Some("the recorded grant carries no list of ungranted directories".to_string());
    }
    None
}

/// Reads the output of `profile status`. A state not known is `unknown`, and a `current`
/// whose lists cannot be read is `outdated`: only a readable `current`
/// decides what may be granted.
pub fn parse_profile_read_status(stdout: &str) -> Result<ProfileReadStatus, WindowsSandboxError> {
    let raw: ProfileReadStatusWire = serde_json::from_str(stdout).map_err(|error| {
        WindowsSandboxError::for_subcommand(
            Code::FenceHelperBadJson,
            format!("fence-helper profile status printed output that is not JSON: {error}"),
            "profile",
        )
    })?;
    let mut state = match raw.state.as_deref() {
        Some("none") => ProfileReadState::None,
        Some("current") => ProfileReadState::Current,
        Some("outdated") => ProfileReadState::Outdated,
        Some("incomplete") => ProfileReadState::Incomplete,
        _ => ProfileReadState::Unknown,
    };
    let mut reason = raw.reason.clone().filter(|text| !text.is_empty());
    if state == ProfileReadState::Current
        && let Some(problem) = status_problem(&raw)
    {
        state = ProfileReadState::Outdated;
        reason = Some(problem);
    }
    let mut env = std::collections::BTreeMap::new();
    if state == ProfileReadState::Current {
        for var in ProfileReadVar::ALL {
            let value = raw
                .env
                .as_ref()
                .and_then(|env| env.get(var.name()))
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty());
            if let Some(value) = value {
                env.insert(var, value.to_string());
            }
        }
    }
    Ok(ProfileReadStatus {
        state,
        reason,
        exclusions: ProfileExclusions {
            exclusions: string_list(raw.exclusions.as_ref()).unwrap_or_default(),
            ungranted_only: string_list(raw.ungranted_only.as_ref()).unwrap_or_default(),
        },
        profile: raw.profile.filter(|text| !text.is_empty()),
        capabilities: text_list("capabilities", raw.capabilities.as_ref())?,
        not_granted: items("not_granted", raw.not_granted)?,
        self_closed: items("self_closed", raw.self_closed)?,
        unverifiable: items("unverifiable", raw.unverifiable)?,
        unprotected: items("unprotected", raw.unprotected)?,
        stale_allow: items("stale_allow", raw.stale_allow)?,
        replaced_files: text_list("replaced_files", raw.replaced_files.as_ref())?,
        extra_exclusions: text_list("extra_exclusions", raw.extra_exclusions.as_ref())?,
        counts: ProfileGrantCounts {
            containers: raw.containers.unwrap_or(0),
            granted: raw.granted.unwrap_or(0),
            files: raw.files.unwrap_or(0),
            denied: raw.denied.unwrap_or(0),
        },
        env,
        paths: raw
            .paths
            .map(|rows| rows.iter().map(grant_path).collect::<Result<Vec<_>, _>>())
            .transpose()?,
    })
}
