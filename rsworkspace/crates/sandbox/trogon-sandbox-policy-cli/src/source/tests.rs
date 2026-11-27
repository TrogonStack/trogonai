use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn home_with(text: Option<&str>) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let home = tempfile::tempdir()?;
    if let Some(text) = text {
        std::fs::write(home.path().join(DEFAULT_FILE_NAME), text)?;
    }
    Ok(home)
}

fn default_source(home: &tempfile::TempDir) -> SettingsSource {
    SettingsSource::choose(None, Some(home.path().to_path_buf()))
}

fn refusal_of(source: &SettingsSource) -> Result<Refusal, Box<dyn std::error::Error>> {
    match source.load(None) {
        Err(refusal) => Ok(refusal),
        Ok(_) => Err("the settings loaded".into()),
    }
}

#[test]
fn an_absent_default_file_means_the_built_in_settings() -> TestResult {
    let home = home_with(None)?;
    assert!(default_source(&home).load(None).is_ok());
    assert!(SettingsSource::choose(None, None).load(None).is_ok());
    Ok(())
}

#[test]
fn a_requested_file_that_is_absent_is_refused() -> TestResult {
    let refusal = refusal_of(&SettingsSource::Requested("/nonexistent/settings.json".into()))?;
    assert!(refusal.reason.contains("does not exist"));
    assert!(refusal.dropped.contains("--settings asked for"));
    Ok(())
}

#[test]
fn a_requested_file_that_is_there_but_does_not_load_names_its_rules() -> TestResult {
    let home = home_with(Some(r#"{"network": {}}"#))?;
    let path = home.path().join(DEFAULT_FILE_NAME);
    let refusal = refusal_of(&SettingsSource::Requested(path.clone()))?;
    assert_eq!(
        refusal,
        Refusal {
            reason: format!(
                "{} does not hold a valid config - network.allowedDomains: Required; \
                 network.deniedDomains: Required; filesystem: Required",
                path.display()
            ),
            dropped: FILE_RULES.to_string(),
        }
    );
    Ok(())
}

#[test]
fn a_refusal_says_what_would_be_dropped() {
    let refusal = Refusal {
        reason: "x is empty.".to_string(),
        dropped: "these rules".to_string(),
    };
    assert_eq!(
        format!("{refusal}"),
        "x is empty.\nRefusing to run with the built-in defaults, which would drop these rules."
    );
}

#[test]
fn a_default_file_that_does_not_load_is_refused() -> TestResult {
    for (text, needle) in [
        ("", "is empty"),
        ("  \n\t ", "is empty"),
        ("{ nope", "is not valid JSON"),
        (r#"{"network": {}}"#, "network.allowedDomains"),
    ] {
        let home = home_with(Some(text))?;
        let refusal = refusal_of(&default_source(&home))?;
        assert!(refusal.reason.contains(needle), "{refusal:?}");
        assert_eq!(refusal.dropped, FILE_RULES);
    }
    Ok(())
}

#[test]
fn a_default_path_that_is_a_directory_is_refused_not_skipped() -> TestResult {
    let home = tempfile::tempdir()?;
    std::fs::create_dir(home.path().join(DEFAULT_FILE_NAME))?;
    let refusal = refusal_of(&default_source(&home))?;
    assert!(refusal.reason.contains("could not be read"));
    assert!(refusal.reason.contains("EISDIR"));
    Ok(())
}
