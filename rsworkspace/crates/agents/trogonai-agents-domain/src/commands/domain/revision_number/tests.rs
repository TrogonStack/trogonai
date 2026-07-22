use super::*;

#[test]
fn rejects_zero() {
    assert_eq!(RevisionNumber::new(0), Err(RevisionNumberError::Zero));
}

#[test]
fn orders_and_displays_by_value() {
    let one = RevisionNumber::GENESIS;
    let two = RevisionNumber::new(2).unwrap();
    assert!(one < two);
    assert_eq!(two.to_string(), "2");
}
