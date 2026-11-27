use super::*;
use crate::render;

#[test]
fn render_is_deterministic_and_covers_every_worker() -> Result<(), MatrixError> {
    let first = render(&columns()?)?;
    assert_eq!(first, render(&columns()?)?);
    for column in columns()? {
        assert!(first.contains(column.label), "missing column {}", column.label);
    }
    Ok(())
}

#[test]
fn labels_are_unique() -> Result<(), MatrixError> {
    let columns = columns()?;
    let mut labels: Vec<&str> = columns.iter().map(|column| column.label).collect();
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(labels.len(), columns.len());
    Ok(())
}
