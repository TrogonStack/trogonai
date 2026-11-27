mod support;

use serde_json::json;
use support::*;
use trogon_sandbox_daytona_worker::WorkspaceArchiveError;

fn box_route(suffix: &str) -> String {
    format!("/box/{SANDBOX}/{suffix}")
}

#[tokio::test]
async fn archive_directory_runs_tar_downloads_and_removes_the_remote_file() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.executes(&[(0, String::new())]);
    fake.raw("GET", &box_route("files/download"), 200, b"tar-bytes");
    fake.json("DELETE", &box_route("files"), 200, json!({}));

    let bytes = fake.api()?.archive_directory(SANDBOX, "/workspace").await?;

    assert_eq!(bytes.as_ref(), b"tar-bytes");
    let executes = fake.calls("POST", &box_route("process/execute"));
    let command = executes
        .first()
        .and_then(|call| {
            call.json()
                .get("command")
                .and_then(|value| value.as_str().map(str::to_owned))
        })
        .ok_or("no tar command")?;
    assert!(command.starts_with("tar -C '/workspace' -cf '/tmp/"), "{command}");
    assert!(command.ends_with(".tar' ."), "{command}");
    assert_eq!(fake.count("DELETE", &box_route("files")), 1);
    Ok(())
}

#[tokio::test]
async fn a_failed_tar_reports_its_exit_code_and_output_without_downloading() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.executes(&[(2, "tar: no such directory".to_owned())]);
    fake.json("DELETE", &box_route("files"), 200, json!({}));

    let result = fake.api()?.archive_directory(SANDBOX, "/missing").await;

    match result {
        Err(WorkspaceArchiveError::TarFailed { exit_code, output }) => {
            assert_eq!(exit_code, 2);
            assert_eq!(output, "tar: no such directory");
        }
        other => return Err(format!("unexpected result: {other:?}").into()),
    }
    assert_eq!(fake.count("GET", &box_route("files/download")), 0);
    Ok(())
}

#[tokio::test]
async fn a_failed_remote_cleanup_does_not_fail_the_archive() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    fake.executes(&[(0, String::new())]);
    fake.raw("GET", &box_route("files/download"), 200, b"tar-bytes");
    fake.json("DELETE", &box_route("files"), 500, json!({"message": "boom"}));

    let bytes = fake.api()?.archive_directory(SANDBOX, "/workspace").await?;

    assert_eq!(bytes.as_ref(), b"tar-bytes");
    Ok(())
}
