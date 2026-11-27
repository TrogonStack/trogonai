use std::path::PathBuf;

use crate::{MarkedDoc, MatrixError, columns, render};

/// The committed document: this crate's `README.md`.
pub fn doc_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md")
}

fn read_doc() -> Result<String, MatrixError> {
    let path = doc_path();
    std::fs::read_to_string(&path).map_err(|source| MatrixError::Read { path, source })
}

fn expected_from(doc: &str) -> Result<String, MatrixError> {
    Ok(MarkedDoc::parse(doc)?.splice(&render(&columns()?)?))
}

/// The committed document with its generated region rendered from the current profiles.
pub fn expected_doc() -> Result<String, MatrixError> {
    expected_from(&read_doc()?)
}

/// Rewrites the generated region of the committed document.
pub fn write_doc() -> Result<(), MatrixError> {
    let expected = expected_doc()?;
    let path = doc_path();
    std::fs::write(&path, expected).map_err(|source| MatrixError::Write { path, source })
}

/// Fails with the regenerate command when the committed document differs from the rendered profiles.
pub fn check_doc() -> Result<(), MatrixError> {
    let actual = read_doc()?;
    if expected_from(&actual)? == actual {
        Ok(())
    } else {
        Err(MatrixError::Stale(doc_path()))
    }
}
