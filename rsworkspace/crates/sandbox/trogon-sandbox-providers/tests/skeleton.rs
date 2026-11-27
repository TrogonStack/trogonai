use trogon_sandbox_providers::{
    ArtifactRef, ArtifactSink, ArtifactSinkError, CommitSha, CommitShaError, GitProvider, GitStep, NoopArtifactSink,
    RepoRef,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn commit_shas_are_parsed_not_validated() -> TestResult {
    let sha = CommitSha::new("abcd")?;
    assert_eq!(sha.as_str(), "abcd");
    assert_eq!(sha.to_string(), "abcd");
    assert_eq!(String::from(sha.clone()), "abcd");
    assert!(matches!(CommitSha::new("abc"), Err(CommitShaError::InvalidLength(3))));
    assert!(matches!(
        CommitSha::new("a".repeat(65)),
        Err(CommitShaError::InvalidLength(65))
    ));
    assert!(matches!(CommitSha::new("wxyz"), Err(CommitShaError::NonHex)));
    assert_eq!(CommitSha::placeholder().as_str(), "0".repeat(40));
    Ok(())
}

#[test]
fn commit_shas_round_trip_through_serde() -> TestResult {
    let sha = CommitSha::new("deadbeef")?;
    let json = serde_json::to_string(&sha)?;
    assert_eq!(json, "\"deadbeef\"");
    assert_eq!(serde_json::from_str::<CommitSha>(&json)?, sha);
    assert!(serde_json::from_str::<CommitSha>("\"zz\"").is_err());
    assert_eq!(CommitSha::try_from("cafe".to_string())?.as_str(), "cafe");
    Ok(())
}

#[test]
fn git_providers_name_themselves() -> TestResult {
    assert_eq!(GitProvider::default(), GitProvider::GitHub);
    assert_eq!(GitProvider::GitHub.as_str(), "github");
    assert_eq!(GitProvider::GitLab.as_str(), "gitlab");
    assert_eq!(GitProvider::GitLab.to_string(), "gitlab");
    assert_eq!(serde_json::to_string(&GitProvider::GitHub)?, "\"github\"");
    Ok(())
}

#[test]
fn repo_refs_display_provider_owner_and_name() {
    let repo = RepoRef {
        provider: GitProvider::GitLab,
        owner: "acme".to_string(),
        name: "widget".to_string(),
    };
    assert_eq!(repo.to_string(), "gitlab:acme/widget");
}

#[test]
fn git_steps_name_themselves() {
    assert_eq!(GitStep::Clone.as_str(), "clone");
    assert_eq!(GitStep::Fetch.as_str(), "fetch");
    assert_eq!(GitStep::Checkout.to_string(), "checkout");
}

#[tokio::test]
async fn the_noop_sink_echoes_puts_and_refuses_gets() -> TestResult {
    let sink = NoopArtifactSink;
    let artifact = sink.put("diff", "out/a.txt", "text/plain", b"hello").await?;
    assert_eq!(
        artifact,
        ArtifactRef {
            tool: "diff".to_string(),
            object_bucket: "noop".to_string(),
            object_key: "out/a.txt".to_string(),
            sha256: String::new(),
            size: 5,
            content_type: "text/plain".to_string(),
        }
    );
    let error = sink.get(&artifact).await.err();
    assert!(matches!(
        error,
        Some(ArtifactSinkError::GetUnsupported { ref object_key }) if object_key == "out/a.txt"
    ));
    Ok(())
}
