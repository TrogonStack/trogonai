use super::WorkspaceTrust;

fn trust(accepted: Option<bool>, directory: Option<&str>) -> WorkspaceTrust {
    WorkspaceTrust {
        accepted,
        directory: directory.map(str::to_owned),
    }
}

#[test]
fn an_accepted_trust_needs_an_absolute_directory_on_either_platform() {
    for directory in ["/work", "C:\\work", "C:/work", "\\\\srv\\share", "\\work"] {
        assert!(trust(Some(true), Some(directory)).validated().is_some(), "{directory}");
    }
    for directory in [Some("C:work"), Some(""), Some("work/x"), None] {
        assert!(trust(Some(true), directory).validated().is_none(), "{directory:?}");
    }
}

#[test]
fn a_declined_or_empty_trust_is_kept_as_given() {
    assert!(trust(Some(false), Some("relative")).validated().is_some());
    assert!(trust(None, None).validated().is_some());
    assert_eq!(trust(None, None).to_object().len(), 0);
}
