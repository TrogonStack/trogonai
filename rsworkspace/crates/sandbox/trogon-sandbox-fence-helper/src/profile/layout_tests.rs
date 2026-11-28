use super::*;

/// A made-up profile with both application-data folders present, as
/// `Layout::resolve` now requires.
pub(super) fn layout_for(prof: &std::path::Path) -> Result<Layout> {
    for rel in [r"AppData\Roaming", r"AppData\Local"] {
        std::fs::create_dir_all(prof.join(rel)).map_err(|e| FenceHelperError::msg(e.to_string()))?;
    }
    Layout::resolve(Some(
        prof.to_str()
            .ok_or_else(|| FenceHelperError::msg("profile path is not UTF-8"))?,
    ))
}

fn scratch_profile(tag: &str) -> std::result::Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let d = std::env::temp_dir().join(format!("fencehelper-layout-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

#[test]
fn a_profile_without_its_appdata_folders_is_an_error() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let prof = scratch_profile("missing")?;
    assert!(Layout::resolve(prof.to_str()).is_err());
    Ok(())
}

#[test]
fn a_profile_with_its_appdata_folders_resolves() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let prof = scratch_profile("present")?;
    let layout = layout_for(&prof)?;
    assert!(layout.roaming.is_some() && layout.local.is_some());
    Ok(())
}
