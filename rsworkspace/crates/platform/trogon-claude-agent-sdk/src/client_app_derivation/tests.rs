use super::{BundledCli, ClientAppDerivation, derive};
use crate::SDK_VERSION;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn bundled(root: &std::path::Path, pinned: &str) -> ClientAppDerivation {
    ClientAppDerivation::Bundled(BundledCli {
        package_root: root.to_path_buf(),
        pinned_version: pinned.to_owned(),
    })
}

fn bundled_cli(root: &std::path::Path) -> Result<std::path::PathBuf, std::io::Error> {
    let directory = root.join("_bundled");
    std::fs::create_dir_all(&directory)?;
    let cli = directory.join(if cfg!(windows) { "claude.exe" } else { "claude" });
    std::fs::write(&cli, "")?;
    Ok(cli)
}

#[test]
fn derives_this_crate_for_a_qualifying_bundled_cli() -> TestResult {
    let root = tempfile::tempdir()?;
    let cli = bundled_cli(root.path())?;
    std::fs::write(cli.with_file_name("claude.version"), "999.1.0\n")?;
    let label = derive(&bundled(root.path(), "999.0.0"), &cli);
    assert_eq!(label, Some(format!("trogon-claude-agent-sdk/{SDK_VERSION}")));
    Ok(())
}

#[test]
fn derives_nothing_unless_every_condition_holds() -> TestResult {
    let root = tempfile::tempdir()?;
    let cli = bundled_cli(root.path())?;
    assert_eq!(derive(&bundled(root.path(), "999.0.0"), &cli), None, "no record");
    std::fs::write(cli.with_file_name("claude.version"), "999.0.0")?;
    assert_eq!(derive(&bundled(root.path(), "2.1.0"), &cli), None, "pin below minimum");
    assert_eq!(
        derive(&bundled(root.path(), "999.0.0"), &root.path().join("claude")),
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
