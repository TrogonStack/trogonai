use super::*;

#[test]
fn the_state_directory_is_under_program_data_and_needs_it() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        state_dir_under(Some("C:\\ProgramData".into()))?,
        PathBuf::from("C:\\ProgramData").join("sandbox")
    );
    let err = state_dir_under(None).err().ok_or("expected a refusal")?;
    assert!(err.to_string().contains("ProgramData is not set"));
    Ok(())
}

fn run(dir: &Path, trusted: Option<&str>, trusted_paths: &mut Vec<String>) -> anyhow::Result<PersistentCa> {
    ensure_persistent_ca(dir, trusted, PersistentCaOptions::default(), &mut |path| {
        trusted_paths.push(std::fs::read_to_string(path)?);
        Ok(())
    })
}

#[test]
fn a_first_run_generates_and_trusts() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut seen = Vec::new();
    let ca = run(&dir.path().join("ca"), None, &mut seen)?;
    assert!(ca.generated && ca.trusted);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0], std::fs::read_to_string(&ca.cert_path)?);
    assert!(ca.key_path.exists());
    let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("ca"))?
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".trust."))
        .collect();
    assert!(leftovers.is_empty());
    Ok(())
}

#[test]
fn a_second_run_reuses_and_trusts_nothing_when_the_status_agrees() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let ca_dir = dir.path().join("ca");
    let first = run(&ca_dir, None, &mut Vec::new())?;
    let mut seen = Vec::new();
    let second = run(&ca_dir, Some(first.thumbprint.as_str()), &mut seen)?;
    assert!(!second.generated && !second.trusted);
    assert!(seen.is_empty());
    assert_eq!(first.thumbprint, second.thumbprint);
    Ok(())
}

#[test]
fn a_stale_status_thumbprint_reconciles_the_trust() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let ca_dir = dir.path().join("ca");
    run(&ca_dir, None, &mut Vec::new())?;
    let mut seen = Vec::new();
    let again = run(&ca_dir, Some("00"), &mut seen)?;
    assert!(!again.generated && again.trusted);
    assert_eq!(seen.len(), 1);
    Ok(())
}

#[test]
fn a_corrupt_store_is_replaced() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let ca_dir = dir.path().join("ca");
    std::fs::create_dir_all(&ca_dir)?;
    std::fs::write(ca_dir.join("ca.json"), "not json")?;
    let ca = run(&ca_dir, None, &mut Vec::new())?;
    assert!(ca.generated);
    Ok(())
}

#[test]
fn a_failed_trust_is_an_error() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let outcome = ensure_persistent_ca(
        &dir.path().join("ca"),
        None,
        PersistentCaOptions::default(),
        &mut |_| anyhow::bail!("seclogon"),
    );
    assert!(outcome.is_err());
    Ok(())
}

fn with(dir: &Path, options: PersistentCaOptions) -> anyhow::Result<PersistentCa> {
    ensure_persistent_ca(dir, None, options, &mut |_| Ok(()))
}

#[test]
fn force_replaces_a_valid_store() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let ca_dir = dir.path().join("ca");
    let first = with(&ca_dir, PersistentCaOptions::default())?;
    let forced = with(
        &ca_dir,
        PersistentCaOptions {
            force: true,
            ..PersistentCaOptions::default()
        },
    )?;
    assert!(forced.generated);
    assert_ne!(first.thumbprint, forced.thumbprint);
    assert_eq!(forced.cert_pem, std::fs::read_to_string(&forced.cert_path)?);
    Ok(())
}

#[test]
fn a_longer_window_replaces_a_store_that_would_expire_inside_it() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let ca_dir = dir.path().join("ca");
    with(&ca_dir, PersistentCaOptions::default())?;
    let renewed = with(
        &ca_dir,
        PersistentCaOptions {
            regenerate_within_days: u32::MAX,
            ..PersistentCaOptions::default()
        },
    )?;
    assert!(renewed.generated);
    Ok(())
}
