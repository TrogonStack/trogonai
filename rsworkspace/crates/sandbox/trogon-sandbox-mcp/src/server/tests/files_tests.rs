use serde_json::{Value, json};
use trogon_sandbox_contract::{FileOp, FileOpResponse, LeaseId, ReadFileResponse, WriteFilesResponse};
use trogon_sandbox_protocol::subjects;

use super::super::file_tools::bounded_watch_wait;
use super::support::{call, call_with};
use crate::policy::{Policy, RootPolicy};
use crate::testing::{Reply, TestResult};

fn rooted() -> Result<Policy, Box<dyn std::error::Error>> {
    Ok(Policy {
        roots: RootPolicy::new(&["/work".to_string()])?,
        ..Policy::default()
    })
}

fn stat_reply(kind: &str) -> Value {
    json!({"outcome": "stat", "stat": {"path": "/work/a", "name": "a", "kind": kind, "size": 0}})
}

#[test]
fn a_watch_wait_is_capped_at_what_the_worker_allows() {
    assert_eq!(bounded_watch_wait(None), None);
    assert_eq!(bounded_watch_wait(Some(5)), Some(5));
    assert_eq!(bounded_watch_wait(Some(u32::MAX)), Some(FileOp::WATCH_MAX_WAIT_MS));
}

#[tokio::test]
async fn every_structured_tool_sends_its_operation() -> TestResult {
    let cases = [
        (
            "sandbox_list_dir",
            json!({"path": "/d", "recursive": true, "max_entries": 3}),
            "list",
        ),
        ("sandbox_stat", json!({"path": "/d"}), "stat"),
        (
            "sandbox_read_range",
            json!({"path": "/d", "offset": 1, "length": 2}),
            "read",
        ),
        (
            "sandbox_edit_file",
            json!({"path": "/d", "find": "a", "replace": "b"}),
            "edit",
        ),
        (
            "sandbox_search_files",
            json!({"path": "/d", "pattern": "x", "regex": true}),
            "search",
        ),
        (
            "sandbox_remove_path",
            json!({"path": "/d", "recursive": true}),
            "remove",
        ),
        ("sandbox_mkdir", json!({"path": "/d"}), "mkdir"),
        (
            "sandbox_rename_path",
            json!({"from": "/a", "to": "/b", "overwrite": true}),
            "rename",
        ),
        (
            "sandbox_archive_get",
            json!({"path": "/d", "max_bytes": 9}),
            "archive_get",
        ),
        (
            "sandbox_archive_put",
            json!({"path": "/d", "archive_base64": "aGk="}),
            "archive_put",
        ),
        ("sandbox_checkpoint", json!({"path": "/d", "hash": true}), "checkpoint"),
        ("sandbox_changes", json!({"since": "start", "path": "/d"}), "changes"),
        ("sandbox_watch_start", json!({"path": "/d"}), "watch_start"),
        (
            "sandbox_watch_poll",
            json!({"watch_id": "w", "wait_ms": 999_999}),
            "watch_poll",
        ),
        ("sandbox_watch_stop", json!({"watch_id": "w"}), "watch_stop"),
    ];
    for (tool, mut args, op) in cases {
        if let Some(object) = args.as_object_mut() {
            object.insert("lease_id".into(), json!("l1"));
        }
        let outcome = call(tool, args, &FileOpResponse::Renamed).await?;
        assert!(!outcome.is_error(), "{tool}: {}", outcome.text()?);
        let sent = outcome.sent.first().ok_or("nothing was sent")?;
        assert_eq!(sent.subject, subjects::lease_file_op(&LeaseId::new("l1")), "{tool}");
        assert_eq!(sent.json()?["op"]["op"], json!(op), "{tool}");
    }
    Ok(())
}

#[tokio::test]
async fn a_watch_poll_wait_is_bounded_on_the_wire() -> TestResult {
    let args = json!({"lease_id": "l1", "watch_id": "w", "wait_ms": 999_999});
    let outcome = call("sandbox_watch_poll", args, &FileOpResponse::Renamed).await?;
    let wait = outcome.sent.first().ok_or("nothing was sent")?.json()?["op"]["wait_ms"].clone();
    assert_eq!(wait, json!(FileOp::WATCH_MAX_WAIT_MS));
    Ok(())
}

#[tokio::test]
async fn provider_refusals_become_tool_errors() -> TestResult {
    let args = json!({"lease_id": "l1", "path": "/d"});
    let unsupported = call(
        "sandbox_stat",
        args.clone(),
        &json!({"outcome": "unsupported", "message": "no"}),
    )
    .await?;
    assert!(unsupported.is_error());
    let missing = call(
        "sandbox_stat",
        args.clone(),
        &json!({"outcome": "not_found", "path": "/d"}),
    )
    .await?;
    assert!(missing.is_error());
    let bad_archive = call(
        "sandbox_archive_put",
        json!({"lease_id": "l1", "path": "/d", "archive_base64": "!!"}),
        &FileOpResponse::Renamed,
    )
    .await?;
    assert!(bad_archive.is_error());
    assert!(bad_archive.sent.is_empty());
    let bad_user = call(
        "sandbox_stat",
        json!({"lease_id": "l1", "path": "/d", "user": "Not A User"}),
        &FileOpResponse::Renamed,
    )
    .await?;
    assert!(bad_user.is_error());
    assert!(bad_user.sent.is_empty());
    Ok(())
}

#[tokio::test]
async fn files_are_written_and_read() -> TestResult {
    let write =
        json!({"lease_id": "l1", "user": "dev", "files": [{"path": "/a", "content_base64": "aGk=", "mode": 420}]});
    let written = call("sandbox_write_files", write, &WriteFilesResponse::Written).await?;
    assert!(!written.is_error());
    let body = written.sent.first().ok_or("nothing was sent")?.json()?;
    assert_eq!(body["files"][0]["content"], json!([104, 105]));
    assert_eq!(body["as_user"], json!("dev"));

    let bad = json!({"lease_id": "l1", "files": [{"path": "/a", "content_base64": "***"}]});
    let rejected = call("sandbox_write_files", bad, &WriteFilesResponse::Written).await?;
    assert!(rejected.is_error());
    assert!(rejected.sent.is_empty());

    let read = call(
        "sandbox_read_file",
        json!({"lease_id": "l1", "path": "/a"}),
        &ReadFileResponse::Content { data: b"hi".to_vec() },
    )
    .await?;
    assert!(read.text()?.contains("aGk="));
    let missing = call(
        "sandbox_read_file",
        json!({"lease_id": "l1", "path": "/a"}),
        &ReadFileResponse::NotFound,
    )
    .await?;
    assert!(missing.is_error());
    Ok(())
}

#[tokio::test]
async fn paths_outside_the_roots_are_refused_before_the_bus() -> TestResult {
    let replies = vec![Reply::json(&FileOpResponse::Renamed)?];
    let outside = call_with(
        rooted()?,
        "sandbox_stat",
        json!({"lease_id": "l1", "path": "/etc/passwd"}),
        replies.clone(),
    )
    .await?;
    assert!(outside.is_error());
    assert!(outside.sent.is_empty());
    let up = call_with(
        rooted()?,
        "sandbox_read_file",
        json!({"lease_id": "l1", "path": "/work/../etc"}),
        replies,
    )
    .await?;
    assert!(up.is_error());
    assert!(up.sent.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_path_inside_the_roots_is_probed_for_symlinks_then_used() -> TestResult {
    let replies = vec![
        Reply::json(&stat_reply("directory"))?,
        Reply::json(&json!({"outcome": "not_found", "path": "/work/a/b"}))?,
        Reply::json(&FileOpResponse::Renamed)?,
    ];
    let args = json!({"lease_id": "l1", "path": "a/b"});
    let outcome = call_with(rooted()?, "sandbox_mkdir", args, replies).await?;
    assert!(!outcome.is_error(), "{}", outcome.text()?);
    assert_eq!(outcome.sent.len(), 3);
    let last = outcome.sent.last().ok_or("nothing was sent")?.json()?;
    assert_eq!(last["op"]["path"], json!("/work/a/b"));
    Ok(())
}

#[tokio::test]
async fn a_symlink_below_a_root_is_refused() -> TestResult {
    let replies = vec![Reply::json(&stat_reply("symlink"))?];
    let args = json!({"lease_id": "l1", "path": "/work/a/b"});
    let outcome = call_with(rooted()?, "sandbox_stat", args, replies).await?;
    assert!(outcome.is_error());
    assert_eq!(outcome.sent.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_probe_that_cannot_prove_the_path_is_refused() -> TestResult {
    let unexpected = vec![Reply::json(&FileOpResponse::Renamed)?];
    let args = json!({"lease_id": "l1", "path": "/work/a"});
    let odd = call_with(rooted()?, "sandbox_stat", args.clone(), unexpected).await?;
    assert!(odd.is_error());
    let silent = call_with(rooted()?, "sandbox_stat", args, vec![Reply::Timeout]).await?;
    assert!(silent.is_error());
    Ok(())
}
