use super::*;

#[test]
fn targets_exist_and_are_absolute() -> Result<()> {
    for t in ambient_deny_targets()? {
        assert!(Path::new(&t).is_dir(), "{t} should exist");
        assert!(t.chars().nth(1) == Some(':'), "{t} should be absolute");
    }
    Ok(())
}

/// On any Windows box the big three resolve and exist.
#[test]
fn core_targets_present() -> Result<()> {
    let ts = ambient_deny_targets()?;
    let has = |frag: &str| ts.iter().any(|t| t.to_ascii_lowercase().contains(frag));
    assert!(has("programdata"), "ProgramData missing from {ts:?}");
    assert!(has("public"), "Public missing from {ts:?}");
    assert!(has(r"\temp"), "Windows\\Temp missing from {ts:?}");
    Ok(())
}

#[test]
fn a_missing_or_empty_root_variable_is_an_error() {
    let full = |name: &str| match name {
        "ProgramData" => Some(r"C:\ProgramData".to_owned()),
        "PUBLIC" => Some(r"C:\Users\Public".to_owned()),
        "SystemRoot" => Some(r"C:\Windows".to_owned()),
        _ => None,
    };
    assert!(ambient_deny_targets_from(full).is_ok());
    for missing in ["ProgramData", "PUBLIC", "SystemRoot"] {
        let without = |name: &str| if name == missing { None } else { full(name) };
        assert!(ambient_deny_targets_from(without).is_err(), "{missing}");
        let empty = |name: &str| {
            if name == missing {
                Some(String::new())
            } else {
                full(name)
            }
        };
        assert!(ambient_deny_targets_from(empty).is_err(), "{missing} empty");
    }
}
