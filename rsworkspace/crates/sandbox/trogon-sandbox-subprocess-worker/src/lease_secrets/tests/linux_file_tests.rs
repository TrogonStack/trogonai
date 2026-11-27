use std::os::unix::fs::MetadataExt;

use super::*;

fn bound_content(secrets: &LeaseSecrets) -> Result<String, Box<dyn Error>> {
    let bind = secrets.binds().first().ok_or("a bound file")?;
    Ok(fs::read_to_string(&bind.host_path)?)
}

fn netrc(path: &Path, no_match: ExtractNoMatch, duplicates: bool) -> Result<SecretBinding, Box<dyn Error>> {
    file_binding(
        "netrc",
        path,
        Tweaks {
            extract: regex(r"password\s+(\S+)")?,
            mask_duplicates: duplicates,
            on_no_match: no_match,
        },
    )
}

#[tokio::test]
async fn file_binding_without_extract_holds_only_the_sentinel() -> TestResult {
    let target = tempfile::tempdir()?;
    let path = target.path().join("token");
    let secrets = prepare(&[file_binding("tok", &path, Tweaks::default())?]).await?;
    let [bind] = secrets.binds() else {
        return Err("one bind expected".into());
    };
    assert!(bind.readonly);
    assert_eq!(bind.sandbox_path, path.to_str().ok_or("utf-8")?);
    let content = fs::read_to_string(&bind.host_path)?;
    assert!(content.starts_with("fake_value_"));
    assert!(!content.contains(&real()));
    Ok(())
}

#[tokio::test]
async fn file_binding_with_extract_keeps_the_rest_of_the_file() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(".netrc");
    fs::write(&path, format!("machine h login u password {}\n", real()))?;
    let secrets = prepare(&[netrc(&path, ExtractNoMatch::Error, false)?]).await?;
    let content = bound_content(&secrets)?;
    assert!(content.starts_with("machine h login u password fake_value_"));
    assert!(content.ends_with('\n'));
    assert!(!content.contains(&real()));
    Ok(())
}

#[tokio::test]
async fn without_mask_duplicates_a_repeated_secret_outside_the_match_stays_real() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(".netrc");
    fs::write(&path, format!("# was {0}\nmachine h password {0}\n", real()))?;
    let secrets = prepare(&[netrc(&path, ExtractNoMatch::Error, false)?]).await?;
    assert!(bound_content(&secrets)?.starts_with(&format!("# was {}\n", real())));
    Ok(())
}

#[tokio::test]
async fn mask_duplicates_replaces_every_occurrence_with_the_bindings_sentinel() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(".netrc");
    fs::write(&path, format!("# was {0}\nmachine h password {0}\n", real()))?;
    let mut secrets = prepare(&[netrc(&path, ExtractNoMatch::Error, true)?]).await?;
    let content = bound_content(&secrets)?;
    assert!(!content.contains(&real()));
    let sentinels: Vec<&str> = content
        .split_whitespace()
        .filter(|w| w.starts_with("fake_value_"))
        .collect();
    assert_eq!(sentinels.len(), 2);
    assert_eq!(sentinels[0], sentinels[1]);
    let registry = secrets.take_handoff().ok_or("a handoff")?.into_registry()?;
    assert_eq!(registry.len(), 1);
    Ok(())
}

#[tokio::test]
async fn mask_duplicates_gives_each_distinct_value_a_sentinel_the_proxy_can_swap_back() -> TestResult {
    let other = format!("tok_{}", "otherdistinctsecret9876543");
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("hosts");
    let original = format!("a: {0}\nb: {1}\n# {1} {0}\n", real(), other);
    fs::write(&path, &original)?;
    let b = file_binding(
        "hosts",
        &path,
        Tweaks {
            extract: regex(r"[ab]:\s*(\S+)")?,
            mask_duplicates: true,
            ..Tweaks::default()
        },
    )?;
    let mut secrets = prepare(&[b]).await?;
    let content = bound_content(&secrets)?;
    assert!(!content.contains(&real()) && !content.contains(&other));
    let registry = secrets.take_handoff().ok_or("a handoff")?.into_registry()?;
    assert_eq!(registry.len(), 2);
    assert_eq!(swapped_back(&registry, &content, "api.example.com")?, original);
    assert_eq!(swapped_back(&registry, &content, "evil.example.com")?, content);
    Ok(())
}

#[tokio::test]
async fn on_no_match_warn_leaves_the_file_unmasked_and_unbound() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("creds");
    fs::write(&path, "nothing to find\n")?;
    let secrets = prepare(&[netrc(&path, ExtractNoMatch::Warn, false)?]).await?;
    assert!(secrets.binds().is_empty());
    assert!(secrets.apply_to_fs_policy(None)?.is_none());
    Ok(())
}

#[tokio::test]
async fn on_no_match_deny_turns_the_file_into_a_read_denial() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("creds");
    fs::write(&path, "nothing to find\n")?;
    let secrets = prepare(&[netrc(&path, ExtractNoMatch::Deny, false)?]).await?;
    assert!(secrets.binds().is_empty());
    let policy = secrets.apply_to_fs_policy(None)?.ok_or("a policy")?;
    assert_eq!(policy.credential_deny_read().len(), 1);
    Ok(())
}

#[tokio::test]
async fn extract_that_finds_nothing_refuses_the_lease() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(".netrc");
    fs::write(&path, "nothing to see\n")?;
    let err = prepare(&[netrc(&path, ExtractNoMatch::Error, false)?])
        .await
        .err()
        .ok_or("the lease should be refused")?
        .to_string();
    assert!(err.contains("matched no credential"), "{err}");
    assert!(!err.contains(&real()));
    Ok(())
}

#[tokio::test]
async fn extract_on_a_missing_or_symlinked_file_refuses_the_lease() -> TestResult {
    let dir = tempfile::tempdir()?;
    let line_rule = || -> Result<Option<Extract>, Box<dyn Error>> { Ok(Some(Extract::EnvLine(EnvVarName::new("K")?))) };
    let missing = file_binding(
        "m",
        &dir.path().join("missing"),
        Tweaks {
            extract: line_rule()?,
            ..Tweaks::default()
        },
    )?;
    assert!(prepare(&[missing]).await.is_err());

    let real_file = dir.path().join("real");
    fs::write(&real_file, "K=v\n")?;
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&real_file, &link)?;
    let linked = file_binding(
        "l",
        &link,
        Tweaks {
            extract: line_rule()?,
            ..Tweaks::default()
        },
    )?;
    assert!(prepare(&[linked]).await.is_err());
    Ok(())
}

#[tokio::test]
async fn fake_copies_live_in_a_private_directory_and_go_with_the_lease() -> TestResult {
    let target = tempfile::tempdir()?;
    let secrets = prepare(&[file_binding("tok", &target.path().join("t"), Tweaks::default())?]).await?;
    let store = secrets.store_dir().ok_or("a store")?.to_path_buf();
    assert_eq!(fs::metadata(&store)?.mode() & 0o777, 0o700);
    let file = &secrets.binds().first().ok_or("a bind")?.host_path;
    assert_eq!(fs::metadata(file)?.mode() & 0o777, 0o600);
    drop(secrets);
    assert!(!store.exists());
    Ok(())
}

#[tokio::test]
async fn jwt_file_binding_masks_only_verified_tokens_and_keeps_the_rest() -> TestResult {
    let token = real_jwt("alice-the-user", "real-signature");
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("creds.json");
    fs::write(
        &path,
        format!("{{\"token\":\"{token}\",\"note\":\"eyJhbc.def.ghi\"}}\n"),
    )?;
    let b = file_binding(
        "jwt",
        &path,
        Tweaks {
            extract: jwt_extract(&[])?,
            ..Tweaks::default()
        },
    )?;
    let secrets = prepare_with(&token, &[b]).await?;
    let content = bound_content(&secrets)?;
    assert!(!content.contains(&token));
    assert!(content.contains("\"note\":\"eyJhbc.def.ghi\""));
    serde_json::from_str::<serde_json::Value>(&content)?;
    Ok(())
}

#[tokio::test]
async fn jwt_file_binding_without_a_verifiable_token_refuses_the_lease() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("creds");
    fs::write(&path, "eyJhbc.def.ghi\n")?;
    let b = file_binding(
        "jwt",
        &path,
        Tweaks {
            extract: jwt_extract(&[])?,
            ..Tweaks::default()
        },
    )?;
    let err = prepare_with(&real_jwt("alice-the-user", "real-signature"), &[b])
        .await
        .err()
        .ok_or("the lease should be refused")?
        .to_string();
    assert!(err.contains("matched no credential"), "{err}");
    Ok(())
}

#[tokio::test]
async fn two_jwts_in_one_file_each_get_their_own_fake_and_claim_sentinels() -> TestResult {
    let alice = real_jwt("alice-the-user", "real-signature");
    let bob = real_jwt("bob-the-builder", "bob-signature");
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("tokens");
    fs::write(&path, format!("a={alice}\nb={bob}\n"))?;
    let b = file_binding(
        "jwt",
        &path,
        Tweaks {
            extract: jwt_extract(&["sub"])?,
            ..Tweaks::default()
        },
    )?;
    let mut secrets = prepare_with(&alice, &[b]).await?;
    let content = bound_content(&secrets)?;
    assert!(!content.contains(&alice) && !content.contains(&bob));
    let fakes: Vec<&str> = content
        .lines()
        .map(|line| line.split_once('=').map(|(_, fake)| fake).ok_or("a key=value line"))
        .collect::<Result<_, _>>()?;
    assert!(fakes.iter().all(|fake| is_jwt(fake)));
    assert_ne!(fakes[0], fakes[1]);
    let registry = secrets.take_handoff().ok_or("a handoff")?.into_registry()?;
    let pairs: Vec<(String, String)> = registry
        .pairs()
        .map(|(sentinel, real)| (sentinel.as_str().to_string(), real.expose().to_string()))
        .collect();
    let real_of = |fake: &str| {
        pairs
            .iter()
            .find(|(sentinel, _)| sentinel == fake)
            .map(|(_, real)| real.as_str())
    };
    assert_eq!(real_of(fakes[0]), Some(alice.as_str()));
    assert_eq!(real_of(fakes[1]), Some(bob.as_str()));
    for claim in ["alice-the-user", "bob-the-builder"] {
        assert!(pairs.iter().any(|(_, real)| real == claim), "{claim}");
    }
    Ok(())
}
