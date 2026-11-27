use super::*;

#[test]
fn a_generated_name_parses_back() -> Result<(), Box<dyn std::error::Error>> {
    let name = ProfileName::generate();
    assert_eq!(ProfileName::parse(name.as_str()), Some(name));
    Ok(())
}

#[test]
fn names_that_are_not_ours_do_not_parse() -> Result<(), Box<dyn std::error::Error>> {
    for text in [
        "",
        "trg-sbx-",
        "trg-sbx-not-a-ulid",
        "other-01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "trg-sbx-01ARZ3NDEKTSV4RRFFQ69G5FAV.txt",
        "..\\trg-sbx-01ARZ3NDEKTSV4RRFFQ69G5FAV",
    ] {
        assert_eq!(ProfileName::parse(text), None, "{text:?}");
    }
    Ok(())
}

#[test]
fn a_recorded_profile_is_left_over_until_it_is_forgotten() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let journal = ProfileJournal::in_root(root.path());
    assert!(journal.leftover().is_empty());

    let name = ProfileName::generate();
    journal.record(&name)?;
    assert_eq!(journal.leftover(), vec![name.clone()]);

    journal.forget(&name);
    assert!(journal.leftover().is_empty());
    Ok(())
}

#[test]
fn files_and_directories_that_are_not_markers_are_ignored() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let journal = ProfileJournal::in_root(root.path());
    let name = ProfileName::generate();
    journal.record(&name)?;
    let directory = root.path().join(DIRECTORY);
    std::fs::write(directory.join("notes.txt"), b"")?;
    std::fs::create_dir(directory.join(ProfileName::generate().as_str()))?;

    assert_eq!(journal.leftover(), vec![name]);
    Ok(())
}
