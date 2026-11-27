use crate::{CommitSha, CommitShaError, job_name};

#[test]
fn the_name_combines_the_repo_identity_and_the_sha_prefix() -> Result<(), CommitShaError> {
    let sha = CommitSha::new("ABCDEF0123456789")?;

    assert_eq!(job_name("Acme", "Widget.rs", &sha), "rs-acme-widget-rs-abcdef01");
    Ok(())
}

#[test]
fn a_sha_shorter_than_the_prefix_contributes_all_of_itself() -> Result<(), CommitShaError> {
    let sha = CommitSha::new("abcd")?;

    assert_eq!(job_name("acme", "widget", &sha), "rs-acme-widget-abcd");
    Ok(())
}

#[test]
fn the_name_is_cut_to_a_dns_label_and_never_ends_with_a_dash() -> Result<(), CommitShaError> {
    let sha = CommitSha::new("0123456789abcdef")?;
    let owner = "o".repeat(40);

    let name = job_name(&owner, "r", &sha);

    assert!(name.len() <= 63);
    assert!(!name.ends_with('-'));

    let trimmed = job_name(&"o".repeat(59), "r", &sha);
    assert_eq!(trimmed.len(), 62);
    assert!(!trimmed.ends_with('-'));
    Ok(())
}

#[test]
fn the_same_commit_always_gives_the_same_name() -> Result<(), CommitShaError> {
    let sha = CommitSha::placeholder();

    assert_eq!(job_name("a", "b", &sha), job_name("a", "b", &sha));
    Ok(())
}
