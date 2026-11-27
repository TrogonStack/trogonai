use trogon_sandbox_lsp::{DocumentPath, DocumentPathError, ProjectDir, ProjectDirError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn project_dir_requires_an_absolute_path_and_normalizes() -> TestResult {
    assert_eq!(
        ProjectDir::new("project"),
        Err(ProjectDirError::NotAbsolute { path: "project".into() })
    );
    assert_eq!(
        ProjectDir::new("/workspace//project/./src/..")?.as_str(),
        "/workspace/project"
    );
    assert_eq!(ProjectDir::new("/workspace/project")?.name(), "project");
    assert_eq!(ProjectDir::new("/")?.name(), "/");
    assert_eq!(ProjectDir::new("/workspace/project")?.to_string(), "/workspace/project");
    Ok(())
}

#[test]
fn project_dir_error_names_the_path() {
    let err = ProjectDirError::NotAbsolute { path: "p".into() };
    assert_eq!(err.to_string(), "project dir must be an absolute path, got \"p\"");
}

#[test]
fn document_path_must_be_relative_and_non_empty() -> TestResult {
    assert_eq!(DocumentPath::new(""), Err(DocumentPathError::Empty));
    assert_eq!(
        DocumentPath::new("/etc/passwd"),
        Err(DocumentPathError::Absolute {
            path: "/etc/passwd".into()
        })
    );
    assert_eq!(DocumentPath::new("src/main.ts")?.as_str(), "src/main.ts");
    assert_eq!(DocumentPath::new("src/main.ts")?.to_string(), "src/main.ts");
    Ok(())
}

#[test]
fn document_path_converts_from_strings() -> TestResult {
    assert_eq!(DocumentPath::try_from("a.ts")?, DocumentPath::new("a.ts")?);
    assert_eq!(
        DocumentPath::try_from(String::from("a.ts"))?,
        DocumentPath::new("a.ts")?
    );
    assert!(DocumentPath::try_from("").is_err());
    assert!(DocumentPath::try_from(String::new()).is_err());
    Ok(())
}

#[test]
fn document_path_error_messages_explain_the_refusal() {
    let empty = format!("{}", DocumentPathError::Empty);
    assert_eq!(empty, "document path is empty");
    let absolute = format!("{}", DocumentPathError::Absolute { path: "/x".into() });
    assert!(absolute.contains("relative"));
}

#[test]
fn resolves_documents_against_the_project_dir() -> TestResult {
    let dir = ProjectDir::new("/workspace/project")?;
    let doc = DocumentPath::new("./src/../main.ts")?;
    assert_eq!(dir.resolve(&doc), "/workspace/project/main.ts");
    assert_eq!(dir.root_uri(), "file:///workspace/project");
    Ok(())
}

#[test]
fn uri_percent_encodes_like_encode_uri_component() -> TestResult {
    let dir = ProjectDir::new("/workspace/my project")?;
    let doc = DocumentPath::new("a b/é#.ts")?;
    assert_eq!(
        dir.document_uri(&doc),
        "file:///workspace/my%20project/a%20b/%C3%A9%23.ts"
    );
    Ok(())
}
