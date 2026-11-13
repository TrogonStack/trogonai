use super::{BundledCli, ClientAppDerivation, derive, meets_derived_minimum};
use crate::SDK_VERSION;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn minimum_version_matches_the_leading_numbers_like_python() {
    for (version, expected) in [
        ("999.0.0", true),
        ("999.0.1\n", true),
        ("1000.0.0-beta", true),
        ("998.9.9", false),
        ("2.1.0", false),
        ("999.0", false),
        ("v999.0.0", false),
        ("1234567890.0.0", false),
        ("", false),
    ] {
        assert_eq!(meets_derived_minimum(version), expected, "{version}");
    }
}

fn bundled(root: &std::path::Path, pinned: &str) -> ClientAppDerivation {
    ClientAppDerivation::Bundled(BundledCli {
        path: root.join("claude"),
        pinned_version: pinned.to_owned(),
    })
}

#[test]
fn derives_this_crate_for_a_qualifying_bundled_cli() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::write(root.path().join("claude.version"), "999.1.0\n")?;
    let label = derive(&bundled(root.path(), "999.0.0"), &root.path().join("claude"));
    assert_eq!(label, Some(format!("trogon-claude-agent-sdk/{SDK_VERSION}")));
    Ok(())
}

#[test]
fn derives_nothing_unless_every_condition_holds() -> TestResult {
    let root = tempfile::tempdir()?;
    let cli = root.path().join("claude");
    assert_eq!(derive(&bundled(root.path(), "999.0.0"), &cli), None, "no record");
    std::fs::write(root.path().join("claude.version"), "999.0.0")?;
    assert_eq!(derive(&bundled(root.path(), "2.1.0"), &cli), None, "pin below minimum");
    assert_eq!(
        derive(&bundled(root.path(), "999.0.0"), &root.path().join("other")),
        None,
        "not the bundled path"
    );
    assert_eq!(
        derive(&ClientAppDerivation::Disabled, &cli),
        None,
        "disabled by default"
    );
    Ok(())
}
