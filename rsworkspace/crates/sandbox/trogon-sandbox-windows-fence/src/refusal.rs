use crate::exit_code::ExitCode;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

const HOLDERS_SHOWN: usize = 8;

fn describe_holder(h: &serde_json::Value) -> String {
    let field = |k: &str| match h.get(k) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Null) | None => "undefined".to_string(),
        Some(v) => v.to_string(),
    };
    let who = format!("process {}", field("pid"));
    let at = field("path");
    match h.get("kind").and_then(|k| k.as_str()) {
        Some("profile") => format!("{who} is a sandbox session or command"),
        Some("kept_root") => format!("{who} runs on the kept write grant on {at}"),
        Some("grant") => format!("{who} holds a session grant on {at}"),
        Some("deny" | "deny_fdc" | "deny_delete" | "deny_satisfied") => {
            format!("{who} holds a session deny on {at}")
        }
        _ => format!("{who} holds {} on {at}", field("kind")),
    }
}

fn describe_holders(stdout: &str) -> String {
    let held: Vec<serde_json::Value> = serde_json::from_str::<serde_json::Value>(stdout)
        .ok()
        .and_then(|v| v.get("held_by").and_then(|h| h.as_array().cloned()))
        .unwrap_or_default();
    let mut said: Vec<String> = held.iter().take(HOLDERS_SHOWN).map(describe_holder).collect();
    if held.len() > said.len() {
        said.push(format!("{} more", held.len() - said.len()));
    }
    said.join("; ")
}

/// The one mapping of `fence-helper`'s refusals beside other sandbox activity:
/// 25 (a profile operation runs), 22 (nothing removed), 24 (grant unchanged).
pub fn refusal_beside_others(args: &[String], status: Option<i32>, stdout: &str) -> Option<WindowsSandboxError> {
    let subcommand = args.first().cloned().unwrap_or_default();
    if ExitCode::PROFILE_BUSY.matches(status) {
        return Some(WindowsSandboxError::for_subcommand(
            Code::ProfileOperationInProgress,
            "The read grant on your user profile is being written or removed right now \
             (fence-helper profile grant / profile forget). Nothing in the sandbox can start until \
             that is done; try again when it has finished.",
            subcommand,
        ));
    }
    let blocked = ExitCode::GRANT_BLOCKED.matches(status);
    if !blocked && !ExitCode::HELD_BY_OTHERS.matches(status) {
        return None;
    }
    let what = describe_holders(stdout);
    let who = if what.is_empty() {
        String::new()
    } else {
        format!(" ({what})")
    };
    if blocked {
        return Some(WindowsSandboxError::for_subcommand(
            Code::ProfileGrantBlockedBySession,
            format!(
                "The read grant on your user profile was not changed: sandbox sessions or \
                 commands are running{who}, and the grant is written only while none is, so that \
                 no session sees it change. What was granted before is as it was. End them and \
                 run it again."
            ),
            subcommand,
        ));
    }
    let head = args.iter().take(2).cloned().collect::<Vec<_>>().join(" ");
    Some(WindowsSandboxError::for_subcommand(
        Code::KeptGrantsInUse,
        format!(
            "fence-helper {head} removed nothing: sandbox sessions or commands are running{who}, and \
             what they run on is not taken from under them. End them and run it again."
        ),
        subcommand,
    ))
}
