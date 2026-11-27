mod support;

use std::sync::Arc;

use serde_json::{Value, json};
use support::TestResult;
use support::fake_aca::FakeAca;
use support::fake_issuer::FakeIssuer;
use support::lease::{clone_of, host_mount, no_auth};
use support::local_backends::LocalBackends;
use trogon_sandbox_aca_worker::{
    AcaError, SessionIdentifier, SessionStatus, VolumeSupport, authenticated_clone_url, clone_git_workspace,
    materialize_workspace, upload_directory, wait_session_ready,
};
use trogon_sandbox_contract::{CloneAuth, PullSecretId, ShortLivedToken, TenantId, WorkspaceSource};
use trogon_sandbox_volumes::StoreResourceName;

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, serde_json::Error> {
    serde_json::from_value(value)
}

fn bearer_auth() -> Value {
    json!({ "bearer_token": { "token_ref": { "literal": { "value": "tok" } } } })
}

fn basic_auth() -> Value {
    json!({ "basic": { "username": "user", "password_ref": { "literal": { "value": "pass" } } } })
}

fn kv_auth() -> Value {
    json!({ "bearer_token": { "token_ref": { "nats_kv": { "bucket": "b", "key": "k" } } } })
}

fn git(repo: &std::path::Path, args: &[&str]) -> TestResult {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "user.name=tester",
            "-c",
            "user.email=tester@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("git {args:?} failed").into())
    }
}

fn repository() -> Result<(tempfile::TempDir, String), Box<dyn std::error::Error>> {
    let repo = tempfile::tempdir()?;
    git(repo.path(), &["init", "-q", "-b", "main"])?;
    std::fs::write(repo.path().join("file.txt"), "hello")?;
    git(repo.path(), &["add", "-A"])?;
    git(repo.path(), &["commit", "-q", "-m", "init"])?;
    let head = std::process::Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["rev-parse", "HEAD"])
        .output()?;
    Ok((repo, String::from_utf8(head.stdout)?.trim().to_owned()))
}

fn support_for(root: &std::path::Path, account: Option<&str>) -> Result<VolumeSupport, Box<dyn std::error::Error>> {
    Ok(VolumeSupport {
        issuer: Arc::new(FakeIssuer::new()),
        backends: Arc::new(LocalBackends {
            root: root.to_path_buf(),
        }),
        storage_account: account.map(StoreResourceName::new).transpose()?,
        credential_id: PullSecretId::new("azure-volume"),
    })
}

#[test]
fn clone_credentials_are_embedded_in_the_url() -> TestResult {
    let url = "https://git.example/team/repo.git";

    assert_eq!(authenticated_clone_url(url, &parse::<CloneAuth>(no_auth())?)?, url);
    assert_eq!(
        authenticated_clone_url(url, &parse::<CloneAuth>(bearer_auth())?)?,
        "https://oauth2:tok@git.example/team/repo.git"
    );
    assert_eq!(
        authenticated_clone_url(url, &parse::<CloneAuth>(basic_auth())?)?,
        "https://user:pass@git.example/team/repo.git"
    );
    Ok(())
}

#[test]
fn clone_secrets_must_be_literals_and_urls_need_a_scheme() -> TestResult {
    let kv = authenticated_clone_url("https://git.example/r.git", &parse::<CloneAuth>(kv_auth())?);
    let schemeless = authenticated_clone_url("git.example/r.git", &parse::<CloneAuth>(bearer_auth())?);

    assert!(matches!(kv, Err(AcaError::Workspace(reason)) if reason.contains("NATS KV")));
    assert!(matches!(schemeless, Err(AcaError::Workspace(reason)) if reason.contains("://")));
    Ok(())
}

#[tokio::test]
async fn a_repository_is_cloned_and_checked_out() -> TestResult {
    let (repo, head) = repository()?;
    let dest = tempfile::tempdir()?;

    clone_git_workspace(
        &format!("file://{}", repo.path().display()),
        &head,
        &parse::<CloneAuth>(no_auth())?,
        &dest.path().join("work"),
    )
    .await?;

    assert_eq!(std::fs::read_to_string(dest.path().join("work/file.txt"))?, "hello");
    Ok(())
}

#[tokio::test]
async fn cloning_a_missing_repository_fails() -> TestResult {
    let dest = tempfile::tempdir()?;

    let error = clone_git_workspace(
        "file:///no/such/repository",
        "HEAD",
        &parse::<CloneAuth>(no_auth())?,
        &dest.path().join("work"),
    )
    .await
    .err();

    assert!(matches!(error, Some(AcaError::Workspace(reason)) if reason.contains("git clone failed")));
    Ok(())
}

#[tokio::test]
async fn checking_out_a_missing_revision_fails() -> TestResult {
    let (repo, _) = repository()?;
    let dest = tempfile::tempdir()?;

    let error = clone_git_workspace(
        &format!("file://{}", repo.path().display()),
        "not-a-revision",
        &parse::<CloneAuth>(no_auth())?,
        &dest.path().join("work"),
    )
    .await
    .err();

    assert!(matches!(error, Some(AcaError::Workspace(reason)) if reason.contains("git checkout failed")));
    Ok(())
}

#[tokio::test]
async fn an_empty_workspace_has_no_files_and_nothing_to_sync() -> TestResult {
    let root = tempfile::tempdir()?;

    let workspace = materialize_workspace(
        &parse::<WorkspaceSource>(json!({ "empty": {} }))?,
        &TenantId::new("acme")?,
        &support_for(root.path(), None)?,
    )
    .await?;

    assert_eq!(std::fs::read_dir(workspace.local_path())?.count(), 0);
    assert!(workspace.volume_sync().is_none());
    Ok(())
}

#[tokio::test]
async fn a_host_mount_workspace_is_refused() -> TestResult {
    let root = tempfile::tempdir()?;

    let error = materialize_workspace(
        &parse::<WorkspaceSource>(host_mount(root.path()))?,
        &TenantId::new("acme")?,
        &support_for(root.path(), None)?,
    )
    .await
    .err();

    assert!(matches!(error, Some(AcaError::InvalidSpec(reason)) if reason.contains("host_mount")));
    Ok(())
}

#[tokio::test]
async fn a_clone_workspace_with_unresolvable_credentials_is_refused() -> TestResult {
    let root = tempfile::tempdir()?;

    let error = materialize_workspace(
        &parse::<WorkspaceSource>(clone_of("https://git.example/r.git", "HEAD", kv_auth()))?,
        &TenantId::new("acme")?,
        &support_for(root.path(), None)?,
    )
    .await
    .err();

    assert!(matches!(error, Some(AcaError::Workspace(_))));
    Ok(())
}

#[tokio::test]
async fn a_volume_workspace_is_pulled_and_a_writable_one_is_synced_back() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::create_dir_all(root.path().join("data"))?;
    std::fs::write(root.path().join("data/file.txt"), "stored")?;
    let volume = |mode: &str| {
        parse::<WorkspaceSource>(
            json!({ "s3_volume": { "bucket": "box", "prefix": "data", "mode": mode, "snapshot": null } }),
        )
    };

    let writable = materialize_workspace(
        &volume("read_write")?,
        &TenantId::new("acme")?,
        &support_for(root.path(), Some("account"))?,
    )
    .await?;
    let readonly = materialize_workspace(
        &volume("read_only")?,
        &TenantId::new("acme")?,
        &support_for(root.path(), Some("account"))?,
    )
    .await?;

    assert_eq!(
        std::fs::read_to_string(writable.local_path().join("file.txt"))?,
        "stored"
    );
    assert!(writable.volume_sync().is_some());
    assert!(readonly.volume_sync().is_none());
    Ok(())
}

#[tokio::test]
async fn an_invalid_volume_name_is_refused() -> TestResult {
    let root = tempfile::tempdir()?;
    let source = parse::<WorkspaceSource>(
        json!({ "s3_volume": { "bucket": "not a valid name!", "prefix": "data", "mode": "read_only", "snapshot": null } }),
    )?;

    let error = materialize_workspace(
        &source,
        &TenantId::new("acme")?,
        &support_for(root.path(), Some("account"))?,
    )
    .await
    .err();

    assert!(matches!(error, Some(AcaError::StoreName(_))));
    Ok(())
}

#[tokio::test]
async fn an_invalid_volume_prefix_is_refused() -> TestResult {
    let root = tempfile::tempdir()?;
    let source = parse::<WorkspaceSource>(
        json!({ "s3_volume": { "bucket": "box", "prefix": "../escape", "mode": "read_only", "snapshot": null } }),
    )?;

    let error = materialize_workspace(
        &source,
        &TenantId::new("acme")?,
        &support_for(root.path(), Some("account"))?,
    )
    .await
    .err();

    assert!(matches!(error, Some(AcaError::Prefix(_))));
    Ok(())
}

#[tokio::test]
async fn every_file_of_a_directory_is_uploaded_relative_to_its_root() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("sub"))?;
    std::fs::write(dir.path().join("b.txt"), "b")?;
    std::fs::write(dir.path().join("sub/a.txt"), "a")?;
    let aca = FakeAca::default();

    let count = upload_directory(
        &aca,
        &ShortLivedToken::new("t"),
        &SessionIdentifier::new("s"),
        dir.path(),
    )
    .await?;

    assert_eq!(count, 2);
    assert_eq!(
        aca.uploads(),
        vec![
            ("b.txt".to_owned(), b"b".to_vec()),
            ("sub/a.txt".to_owned(), b"a".to_vec())
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_upload_stops_the_directory_upload() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("a.txt"), "a")?;
    let aca = FakeAca::default();
    aca.fail_uploads();

    let error = upload_directory(
        &aca,
        &ShortLivedToken::new("t"),
        &SessionIdentifier::new("s"),
        dir.path(),
    )
    .await
    .err();

    assert!(matches!(error, Some(AcaError::AcaApi { status: 500, .. })));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn waiting_ends_on_an_error_other_than_a_missing_session() -> TestResult {
    let aca = FakeAca::default();
    aca.script_get(Err(404));
    aca.script_get(Ok(SessionStatus::Pending("Creating".to_owned())));
    aca.script_get(Err(503));

    let error = wait_session_ready(
        &aca,
        &ShortLivedToken::new("t"),
        &SessionIdentifier::new("s"),
        10,
        std::time::Duration::from_millis(10),
    )
    .await
    .err();

    assert!(matches!(error, Some(AcaError::AcaApi { status: 503, .. })));
    Ok(())
}
