use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use trogon_sandbox_contract::{FileContentHashing, FileOp, FileOpErrorKind, FileOpResponse, FileRecursion};
use trogon_sandbox_file_ops::{FileOpError, ManifestSource, Tracking};
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::constants::MAX_MANIFEST_BYTES;
use super::{EnvdSource, record_start_baseline};
use crate::api::E2bConfig;
use crate::envd::{Envd, EnvdTarget};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const STAT_FILE: &str = "regular file|3|2024-01-01 00:00:00.000000000 +0000|/w/a.txt";
const STAT_DIR: &str = "directory|4096|2024-01-01 00:00:00.000000000 +0000|/w/d";
const DIGEST: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

fn frame(flags: u8, value: &Value) -> Result<Vec<u8>, serde_json::Error> {
    let body = serde_json::to_vec(value)?;
    let mut out = vec![flags];
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// A finished envd process: `stdout`, `stderr` and the exit code.
fn finished(stdout: &[u8], stderr: &str, exit: i32) -> Result<Vec<u8>, serde_json::Error> {
    let mut out = frame(
        0,
        &json!({"event": {"data": {"stdout": STANDARD.encode(stdout), "stderr": STANDARD.encode(stderr)}}}),
    )?;
    out.extend(frame(
        0,
        &json!({"event": {"end": {"exited": true, "exitCode": exit}}}),
    )?);
    out.extend(frame(0b10, &json!({}))?);
    Ok(out)
}

fn script_of(request: &Request) -> String {
    String::from_utf8_lossy(&request.body).into_owned()
}

fn is_hash_script(request: &Request) -> bool {
    script_of(request).contains("sha256sum")
}

async fn mount_manifest(server: &MockServer, reply: Vec<u8>) {
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .and(|request: &Request| !is_hash_script(request))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(reply))
        .mount(server)
        .await;
}

async fn mount_hashes(server: &MockServer, reply: Vec<u8>) {
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .and(is_hash_script)
        .respond_with(ResponseTemplate::new(200).set_body_bytes(reply))
        .mount(server)
        .await;
}

fn envd_for(server: &MockServer) -> Result<(Envd, EnvdTarget), Box<dyn std::error::Error>> {
    let config = E2bConfig::new(Url::parse("https://api.e2b.app")?, "e2b.app", "k")?
        .with_envd_url(Url::parse(&server.uri())?)?;
    let target = EnvdTarget::for_sandbox(&config, "sbx1", None, Some("tok".to_owned()))?;
    Ok((Envd::new(Duration::from_secs(5))?, target))
}

async fn scan(
    server: &MockServer,
    path: &str,
    recursion: FileRecursion,
    hashing: FileContentHashing,
) -> Result<trogon_sandbox_file_ops::Manifest, Box<dyn std::error::Error>> {
    let (envd, target) = envd_for(server)?;
    let source = EnvdSource::new(&envd, &target, "user");
    Ok(source.manifest(path, recursion, hashing).await?)
}

async fn scan_error(
    server: &MockServer,
    path: &str,
    hashing: FileContentHashing,
) -> Result<FileOpError, Box<dyn std::error::Error>> {
    let (envd, target) = envd_for(server)?;
    let source = EnvdSource::new(&envd, &target, "user");
    match source.manifest(path, FileRecursion::Recursive, hashing).await {
        Ok(_) => Err("the scan should have failed".into()),
        Err(error) => Ok(error),
    }
}

fn kind_of(error: &FileOpError) -> Option<FileOpErrorKind> {
    match error {
        FileOpError::Failed { kind, .. } => Some(*kind),
        _ => None,
    }
}

#[tokio::test]
async fn a_scan_reads_files_and_directories() -> TestResult {
    let server = MockServer::start().await;
    mount_manifest(
        &server,
        finished(format!("{STAT_FILE}\n{STAT_DIR}\n").as_bytes(), "", 0)?,
    )
    .await;
    let manifest = scan(&server, "/w", FileRecursion::Recursive, FileContentHashing::Metadata).await?;
    assert_eq!(manifest.len(), 2);
    assert!(manifest.get("/w/d").is_some_and(|entry| entry.is_directory()));
    assert_eq!(manifest.get("/w/a.txt").map(|entry| entry.size()), Some(3));
    assert!(manifest.get("/w/a.txt").is_some_and(|entry| entry.hash().is_none()));
    Ok(())
}

#[tokio::test]
async fn the_scan_script_gets_the_path_the_flag_and_the_ceilings() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .and(|request: &Request| {
            let body = script_of(request);
            body.contains("\"/w\"")
                && body.contains("\"0\"")
                && body.contains(&format!("\"{}\"", MAX_MANIFEST_BYTES + 1))
        })
        .respond_with(ResponseTemplate::new(200).set_body_bytes(finished(b"", "", 0)?))
        .expect(1)
        .mount(&server)
        .await;
    let manifest = scan(&server, "/w", FileRecursion::Flat, FileContentHashing::Metadata).await?;
    assert!(manifest.is_empty());
    Ok(())
}

#[tokio::test]
async fn content_hashes_are_attached_to_files_only() -> TestResult {
    let server = MockServer::start().await;
    mount_manifest(
        &server,
        finished(format!("{STAT_FILE}\n{STAT_DIR}\n").as_bytes(), "", 0)?,
    )
    .await;
    let hashes =
        format!("{DIGEST}  /w/a.txt\n{DIGEST}  /w/d\n{DIGEST}  /w/unknown\nnot a hash line\nnothex  /w/a.txt\n");
    mount_hashes(&server, finished(hashes.as_bytes(), "", 0)?).await;
    let manifest = scan(&server, "/w", FileRecursion::Recursive, FileContentHashing::Content).await?;
    assert_eq!(manifest.len(), 2);
    assert_eq!(
        manifest
            .get("/w/a.txt")
            .and_then(|entry| entry.hash())
            .map(|hash| hash.as_str()),
        Some(DIGEST)
    );
    assert_eq!(manifest.get("/w/a.txt").map(|entry| entry.size()), Some(3));
    assert!(manifest.get("/w/d").is_some_and(|entry| entry.hash().is_none()));
    Ok(())
}

#[tokio::test]
async fn a_relative_path_and_the_root_are_refused() -> TestResult {
    let server = MockServer::start().await;
    let relative = scan_error(&server, "w", FileContentHashing::Metadata).await?;
    assert_eq!(kind_of(&relative), Some(FileOpErrorKind::InvalidPath));
    let root = scan_error(&server, "/", FileContentHashing::Metadata).await?;
    assert_eq!(kind_of(&root), Some(FileOpErrorKind::InvalidPath));
    Ok(())
}

#[tokio::test]
async fn a_missing_path_is_not_found() -> TestResult {
    let server = MockServer::start().await;
    mount_manifest(&server, finished(b"", "", 44)?).await;
    let error = scan_error(&server, "/gone", FileContentHashing::Metadata).await?;
    assert_eq!(
        error,
        FileOpError::NotFound {
            path: "/gone".to_owned()
        }
    );
    Ok(())
}

#[tokio::test]
async fn a_failing_scan_reports_its_exit_code_and_stderr() -> TestResult {
    let server = MockServer::start().await;
    mount_manifest(&server, finished(b"", " boom \n", 2)?).await;
    let error = scan_error(&server, "/w", FileContentHashing::Metadata).await?;
    assert_eq!(kind_of(&error), Some(FileOpErrorKind::Io));
    assert!(matches!(&error, FileOpError::Failed { message, .. } if message.contains("exited 2: boom")));
    Ok(())
}

#[tokio::test]
async fn an_envd_failure_is_mapped_to_a_file_op_error() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"code": "unauthenticated", "message": "no"})))
        .mount(&server)
        .await;
    let error = scan_error(&server, "/w", FileContentHashing::Metadata).await?;
    assert_eq!(kind_of(&error), Some(FileOpErrorKind::PermissionDenied));
    Ok(())
}

#[tokio::test]
async fn a_listing_past_the_byte_ceiling_is_too_large() -> TestResult {
    let server = MockServer::start().await;
    mount_manifest(&server, finished(&vec![b'x'; MAX_MANIFEST_BYTES + 1], "", 0)?).await;
    let error = scan_error(&server, "/w", FileContentHashing::Metadata).await?;
    assert_eq!(kind_of(&error), Some(FileOpErrorKind::TooLarge));
    Ok(())
}

#[tokio::test]
async fn a_listing_past_the_entry_ceiling_is_too_large() -> TestResult {
    let server = MockServer::start().await;
    let listing: String = (0..=FileOp::TRACKING_MAX_ENTRIES)
        .map(|index| format!("regular file|1|t|/w/{index}\n"))
        .collect();
    mount_manifest(&server, finished(listing.as_bytes(), "", 0)?).await;
    let error = scan_error(&server, "/w", FileContentHashing::Metadata).await?;
    assert_eq!(kind_of(&error), Some(FileOpErrorKind::TooLarge));
    Ok(())
}

#[tokio::test]
async fn a_sandbox_without_sha256sum_cannot_hash() -> TestResult {
    let server = MockServer::start().await;
    mount_manifest(&server, finished(STAT_FILE.as_bytes(), "", 0)?).await;
    mount_hashes(&server, finished(b"", "", 127)?).await;
    let error = scan_error(&server, "/w", FileContentHashing::Content).await?;
    assert!(matches!(error, FileOpError::Unsupported { .. }));
    Ok(())
}

#[tokio::test]
async fn a_hash_listing_past_the_byte_ceiling_is_too_large() -> TestResult {
    let server = MockServer::start().await;
    mount_manifest(&server, finished(STAT_FILE.as_bytes(), "", 0)?).await;
    mount_hashes(&server, finished(&vec![b'x'; MAX_MANIFEST_BYTES + 1], "", 0)?).await;
    let error = scan_error(&server, "/w", FileContentHashing::Content).await?;
    assert_eq!(kind_of(&error), Some(FileOpErrorKind::TooLarge));
    Ok(())
}

async fn changes_after_baseline(
    tracking: &Tracking,
    lease: &str,
    server: &MockServer,
) -> Result<Option<FileOpResponse>, Box<dyn std::error::Error>> {
    let (envd, target) = envd_for(server)?;
    let source = EnvdSource::new(&envd, &target, "user");
    let op = FileOp::Changes {
        since: None,
        path: None,
    };
    Ok(tracking.serve(lease, &op, &source).await)
}

#[tokio::test]
async fn the_start_baseline_is_recorded_and_diffed_against_a_later_scan() -> TestResult {
    let before = MockServer::start().await;
    mount_manifest(&before, finished(b"directory|0|t|/home/user/a\n", "", 0)?).await;
    let after = MockServer::start().await;
    mount_manifest(
        &after,
        finished(b"directory|0|t|/home/user/a\ndirectory|0|t|/home/user/b\n", "", 0)?,
    )
    .await;
    let tracking = Tracking::new();
    let (envd, target) = envd_for(&before)?;
    record_start_baseline(&tracking, "lease", &envd, &target).await;
    let Some(FileOpResponse::Changes { added, since, .. }) = changes_after_baseline(&tracking, "lease", &after).await?
    else {
        return Err("changes expected".into());
    };
    assert_eq!(added, vec!["/home/user/b".to_owned()]);
    assert_eq!(since, FileOpResponse::CHANGES_SINCE_START);
    Ok(())
}

#[tokio::test]
async fn a_failed_baseline_scan_leaves_the_lease_without_one() -> TestResult {
    let good = MockServer::start().await;
    mount_manifest(&good, finished(b"", "", 0)?).await;
    let bad = MockServer::start().await;
    mount_manifest(&bad, finished(b"", "", 44)?).await;
    let tracking = Tracking::new();
    let (envd, target) = envd_for(&good)?;
    record_start_baseline(&tracking, "lease", &envd, &target).await;
    let (envd, target) = envd_for(&bad)?;
    record_start_baseline(&tracking, "lease", &envd, &target).await;
    let Some(FileOpResponse::Failed { kind, .. }) = changes_after_baseline(&tracking, "lease", &good).await? else {
        return Err("a failure expected".into());
    };
    assert_eq!(kind, FileOpErrorKind::InvalidRequest);
    Ok(())
}
