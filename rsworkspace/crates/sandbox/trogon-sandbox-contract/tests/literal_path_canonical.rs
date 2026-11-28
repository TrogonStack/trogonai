use trogon_sandbox_contract::{EnvVarName, LiteralPath, SecretName};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn slash_runs_dot_components_and_trailing_runs_collapse() -> TestResult {
    for (raw, want) in [
        ("/abs//x", "/abs/x"),
        ("/abs///x", "/abs/x"),
        ("/abs/./x", "/abs/x"),
        ("/abs/././x", "/abs/x"),
        ("/abs/x/.", "/abs/x"),
        ("/abs/.//x//", "/abs/x"),
        ("//abs/x", "/abs/x"),
        ("//", "/"),
        ("/.", "/"),
        ("/", "/"),
        ("/data/secrets/", "/data/secrets"),
        ("/data/secrets//", "/data/secrets"),
    ] {
        assert_eq!(LiteralPath::new(raw)?.as_str(), want, "{raw}");
    }
    Ok(())
}

#[test]
fn dot_file_segments_are_kept() -> TestResult {
    for path in ["/abs/.ssh/id_rsa", "/abs/.../b", "/abs/.*/x"] {
        assert_eq!(LiteralPath::new(path)?.as_str(), path);
    }
    Ok(())
}

#[test]
fn parent_segments_fold_lexically_across_several_levels() -> TestResult {
    for (raw, want) in [("/abs/sub/../x", "/abs/x"), ("/a/b/../../c", "/c"), ("/a/..", "/")] {
        assert_eq!(LiteralPath::new(raw)?.as_str(), want, "{raw}");
    }
    Ok(())
}

#[test]
fn secret_and_env_var_names_have_no_upper_length_limit() -> TestResult {
    assert_eq!(EnvVarName::new("A".repeat(300))?.as_str().len(), 300);
    assert_eq!(SecretName::new("a".repeat(300))?.as_str().len(), 300);
    Ok(())
}
