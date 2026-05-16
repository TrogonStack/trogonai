//! Trigger parsing and event matching.
//!
//! Trigger format: `"<nats-subject>:<action>"` or just `"<nats-subject>"`.
//!
//! Subject matching is exact for GitHub subjects.  For `"linear.Issue"` any
//! NATS subject that *starts with* `"linear.Issue"` matches (because the
//! trogon-linear publisher uses subjects like `linear.Issue.create`).

use serde_json::Value;

/// Parse `"subject:action"` into `(&str, Option<&str>)`.
///
/// If there is no `:`, the action part is `None` (matches any action).
pub fn parse(trigger: &str) -> (&str, Option<&str>) {
    match trigger.split_once(':') {
        Some((subj, action)) => (subj, Some(action)),
        None => (trigger, None),
    }
}

/// Return `true` when `trigger` matches the given NATS subject and event payload.
///
/// Subject matching:
/// - `"linear.Issue"` matches any subject that starts with `"linear.Issue"`.
/// - `"cron"` matches any subject that starts with `"cron."`.
/// - `"cron.my-job-id"` matches that exact cron subject.
/// - All other subjects require an exact match.
///
/// Action matching: if the trigger includes an action the payload `"action"`
/// field must equal it (case-sensitive), with one synthetic action:
/// - `"draft_opened"` matches `action == "opened"` **and**
///   `pull_request.draft == true`.
pub fn matches(trigger: &str, nats_subject: &str, payload: &Value) -> bool {
    let (subj, action_filter) = parse(trigger);

    let subject_ok = if subj == "linear.Issue" {
        nats_subject.starts_with("linear.Issue")
    } else if subj == "cron" {
        nats_subject.starts_with("cron.")
    } else {
        nats_subject == subj
    };

    if !subject_ok {
        return false;
    }

    match action_filter {
        None => true,
        Some("draft_opened") => {
            payload["action"].as_str() == Some("opened")
                && payload["pull_request"]["draft"].as_bool() == Some(true)
        }
        // "pushed" is the UI label for GitHub's "synchronize" action (new commits pushed to PR).
        Some("pushed") => payload["action"].as_str() == Some("synchronize"),
        Some(required) => payload["action"].as_str() == Some(required),
    }
}

/// Return `Err` if `trigger` has an invalid format.
///
/// Rules:
///   - Must not be empty.
///   - Must not contain whitespace.
///   - If it contains `:`, neither subject nor action can be empty.
pub fn validate(trigger: &str) -> Result<(), String> {
    if trigger.is_empty() {
        return Err("trigger must not be empty".to_string());
    }
    if trigger.chars().any(|c| c.is_whitespace()) {
        return Err(format!("trigger must not contain whitespace: {trigger:?}"));
    }
    if let Some((subj, action)) = trigger.split_once(':') {
        if subj.is_empty() {
            return Err("trigger subject must not be empty".to_string());
        }
        if action.is_empty() {
            return Err(
                "trigger action must not be empty (omit `:` to match any action)".to_string(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_with_action() {
        assert_eq!(
            parse("github.pull_request:opened"),
            ("github.pull_request", Some("opened"))
        );
    }

    #[test]
    fn parse_without_action() {
        assert_eq!(parse("github.push"), ("github.push", None));
    }

    #[test]
    fn parse_linear_with_action() {
        assert_eq!(
            parse("linear.Issue:create"),
            ("linear.Issue", Some("create"))
        );
    }

    #[test]
    fn matches_exact_subject_no_action_filter() {
        assert!(matches(
            "github.push",
            "github.push",
            &json!({"ref": "refs/heads/main"})
        ));
        assert!(!matches("github.push", "github.pull_request", &json!({})));
    }

    #[test]
    fn matches_with_action_filter() {
        let opened = json!({"action": "opened"});
        assert!(matches(
            "github.pull_request:opened",
            "github.pull_request",
            &opened
        ));
        assert!(!matches(
            "github.pull_request:opened",
            "github.pull_request",
            &json!({"action": "closed"})
        ));
    }

    #[test]
    fn no_action_in_trigger_matches_any_action() {
        assert!(matches(
            "github.pull_request",
            "github.pull_request",
            &json!({"action": "opened"})
        ));
        assert!(matches(
            "github.pull_request",
            "github.pull_request",
            &json!({"action": "closed"})
        ));
        assert!(matches(
            "github.pull_request",
            "github.pull_request",
            &json!({})
        ));
    }

    #[test]
    fn matches_linear_prefix_with_action() {
        let payload = json!({"action": "create", "type": "Issue"});
        assert!(matches(
            "linear.Issue:create",
            "linear.Issue.create",
            &payload
        ));
        assert!(matches("linear.Issue:create", "linear.Issue", &payload));
        assert!(!matches(
            "linear.Issue:create",
            "linear.Issue.create",
            &json!({"action": "update"})
        ));
    }

    #[test]
    fn matches_linear_prefix_no_action() {
        assert!(matches("linear.Issue", "linear.Issue", &json!({})));
        assert!(matches("linear.Issue", "linear.Issue.create", &json!({})));
        assert!(!matches("linear.Issue", "linear.Comment", &json!({})));
    }

    #[test]
    fn wrong_subject_never_matches() {
        assert!(!matches("github.push", "github.check_run", &json!({})));
        assert!(!matches("github.issue_comment", "github.push", &json!({})));
    }

    // ── Cron triggers ─────────────────────────────────────────────────────────

    #[test]
    fn cron_wildcard_matches_any_cron_subject() {
        assert!(matches("cron", "cron.daily-digest", &json!({})));
        assert!(matches("cron", "cron.nightly-cleanup", &json!({})));
    }

    #[test]
    fn cron_wildcard_does_not_match_non_cron_subject() {
        assert!(!matches("cron", "github.push", &json!({})));
        assert!(!matches("cron", "linear.Issue.create", &json!({})));
    }

    #[test]
    fn cron_exact_matches_that_job_only() {
        assert!(matches("cron.my-job", "cron.my-job", &json!({})));
        assert!(!matches("cron.my-job", "cron.other-job", &json!({})));
    }

    #[test]
    fn cron_bare_does_not_match_bare_cron_subject() {
        // "cron" (no dot) must NOT match a subject literally named "cron".
        assert!(!matches("cron", "cron", &json!({})));
    }

    // ── draft_opened synthetic action ─────────────────────────────────────────

    #[test]
    fn draft_opened_matches_opened_draft_pr() {
        let payload = json!({"action": "opened", "pull_request": {"draft": true}});
        assert!(matches(
            "github.pull_request:draft_opened",
            "github.pull_request",
            &payload
        ));
    }

    #[test]
    fn draft_opened_does_not_match_non_draft_pr() {
        let payload = json!({"action": "opened", "pull_request": {"draft": false}});
        assert!(!matches(
            "github.pull_request:draft_opened",
            "github.pull_request",
            &payload
        ));
    }

    #[test]
    fn draft_opened_does_not_match_closed_draft_pr() {
        let payload = json!({"action": "closed", "pull_request": {"draft": true}});
        assert!(!matches(
            "github.pull_request:draft_opened",
            "github.pull_request",
            &payload
        ));
    }

    // ── pushed synthetic action ────────────────────────────────────────────────

    #[test]
    fn pushed_matches_synchronize_action() {
        let payload = json!({"action": "synchronize", "pull_request": {"number": 42}});
        assert!(matches(
            "github.pull_request:pushed",
            "github.pull_request",
            &payload
        ));
    }

    #[test]
    fn pushed_does_not_match_opened_action() {
        let payload = json!({"action": "opened"});
        assert!(!matches(
            "github.pull_request:pushed",
            "github.pull_request",
            &payload
        ));
    }

    #[test]
    fn pushed_does_not_match_wrong_subject() {
        let payload = json!({"action": "synchronize"});
        assert!(!matches(
            "github.pull_request:pushed",
            "github.push",
            &payload
        ));
    }

    #[test]
    fn draft_opened_does_not_match_missing_draft_field() {
        // If the draft field is absent, it must not match.
        let payload = json!({"action": "opened", "pull_request": {}});
        assert!(!matches(
            "github.pull_request:draft_opened",
            "github.pull_request",
            &payload
        ));
    }

    // ── validate ──────────────────────────────────────────────────────────────

    #[test]
    fn validate_rejects_empty() {
        assert!(validate("").is_err());
    }

    #[test]
    fn validate_rejects_whitespace() {
        assert!(validate("github.push opened").is_err());
        assert!(validate("github.push\t").is_err());
    }

    #[test]
    fn validate_rejects_empty_action() {
        assert!(validate("github.push:").is_err());
    }

    #[test]
    fn validate_rejects_empty_subject() {
        assert!(validate(":opened").is_err());
    }

    #[test]
    fn validate_accepts_valid_triggers() {
        assert!(validate("github.push").is_ok());
        assert!(validate("github.pull_request:opened").is_ok());
        assert!(validate("cron").is_ok());
        assert!(validate("cron.my-job-id").is_ok());
        assert!(validate("linear.Issue:create").is_ok());
    }
}
