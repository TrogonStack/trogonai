use super::{PathVetoError, RootPolicy, RootPolicyError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn s(items: &[&str]) -> Vec<String> {
    items.iter().map(|i| (*i).to_string()).collect()
}

fn roots(items: &[&str]) -> Result<RootPolicy, RootPolicyError> {
    RootPolicy::new(&s(items))
}

#[test]
fn paths_under_a_root_are_accepted_and_normalized() -> TestResult {
    let policy = roots(&["/workspace", "/tmp/scratch"])?;
    let confined = policy.confine("/workspace/./src//main.rs")?;
    assert_eq!(confined.path, "/workspace/src/main.rs");
    assert_eq!(confined.below_root, s(&["/workspace/src", "/workspace/src/main.rs"]));
    assert!(policy.confine("/tmp/scratch")?.below_root.is_empty());
    Ok(())
}

#[test]
fn relative_paths_resolve_against_the_first_root() -> TestResult {
    let policy = roots(&["/workspace", "/other"])?;
    assert_eq!(policy.confine("a/b.txt")?.path, "/workspace/a/b.txt");
    assert_eq!(policy.confine(".")?.path, "/workspace");
    Ok(())
}

#[test]
fn parent_components_are_refused_even_when_they_stay_inside() -> TestResult {
    let policy = roots(&["/workspace"])?;
    for path in ["/workspace/../etc/passwd", "../x", "/workspace/a/../b", "a/.."] {
        assert!(
            matches!(policy.confine(path), Err(PathVetoError::ParentComponent(_))),
            "{path}"
        );
    }
    Ok(())
}

#[test]
fn paths_outside_every_root_are_refused() -> TestResult {
    let policy = roots(&["/workspace"])?;
    for path in ["/etc/passwd", "/workspace-evil/x", "/", "/work"] {
        assert!(
            matches!(policy.confine(path), Err(PathVetoError::OutsideRoots(_))),
            "{path}"
        );
    }
    assert_eq!(policy.confine(""), Err(PathVetoError::Empty));
    Ok(())
}

#[test]
fn a_relative_path_with_no_roots_is_outside() {
    assert_eq!(
        RootPolicy::default().confine("a"),
        Err(PathVetoError::OutsideRoots("a".to_string()))
    );
}

#[test]
fn the_deepest_matching_root_wins() -> TestResult {
    let policy = roots(&["/w", "/w/inner"])?;
    assert_eq!(policy.confine("/w/inner/x")?.below_root, s(&["/w/inner/x"]));
    Ok(())
}

#[test]
fn roots_must_be_absolute_and_free_of_parent_components() -> TestResult {
    assert_eq!(
        roots(&["workspace"]).err(),
        Some(RootPolicyError::NotAbsolute("workspace".into()))
    );
    assert_eq!(
        roots(&["/a/../b"]).err(),
        Some(RootPolicyError::ParentComponent("/a/../b".into()))
    );
    assert!(roots(&[])?.is_empty());
    Ok(())
}

#[test]
fn veto_messages_name_the_path() {
    for veto in [
        PathVetoError::Empty,
        PathVetoError::ParentComponent("p".into()),
        PathVetoError::OutsideRoots("p".into()),
        PathVetoError::Symlink("p".into()),
        PathVetoError::Unverified {
            path: "p".into(),
            reason: "r".into(),
        },
    ] {
        assert!(!veto.to_string().is_empty());
    }
}
