use super::*;

#[test]
fn splice_replaces_only_the_marked_region() -> Result<(), MatrixError> {
    let doc = format!("a\n{BEGIN_MARKER}\nold\n{END_MARKER}\nz\n");
    let out = MarkedDoc::parse(&doc)?.splice("new\n");
    assert_eq!(out, format!("a\n{BEGIN_MARKER}\n\nnew\n\n{END_MARKER}\nz\n"));
    Ok(())
}

#[test]
fn splice_is_idempotent() -> Result<(), MatrixError> {
    let doc = format!("a\n{BEGIN_MARKER}\nold\n{END_MARKER}\nz\n");
    let once = MarkedDoc::parse(&doc)?.splice("new\n");
    let twice = MarkedDoc::parse(&once)?.splice("new\n");
    assert_eq!(once, twice);
    Ok(())
}

#[test]
fn a_missing_begin_marker_is_refused() {
    let doc = format!("a\n{END_MARKER}\n");
    assert!(matches!(
        MarkedDoc::parse(&doc),
        Err(MatrixError::MissingMarker(marker)) if marker == BEGIN_MARKER
    ));
}

#[test]
fn a_missing_end_marker_is_refused() {
    let doc = format!("a\n{BEGIN_MARKER}\n");
    assert!(matches!(
        MarkedDoc::parse(&doc),
        Err(MatrixError::MissingMarker(marker)) if marker == END_MARKER
    ));
}

#[test]
fn markers_out_of_order_are_refused() {
    let doc = format!("{END_MARKER}\n{BEGIN_MARKER}\n");
    assert!(matches!(MarkedDoc::parse(&doc), Err(MatrixError::MarkersOutOfOrder)));
}
