use super::*;

#[test]
fn refuses_an_empty_value() {
    assert_eq!(SealedValue::new(Vec::new()), Err(SealedValueError::Empty));
    assert_eq!(SealedValueError::Empty.to_string(), "sealed value must not be empty");
}

#[test]
fn keeps_the_sealed_bytes() {
    let sealed = SealedValue::new(vec![1, 2, 3]).unwrap();

    assert_eq!(sealed.as_bytes(), &[1, 2, 3]);
    assert_eq!(sealed.into_bytes(), vec![1, 2, 3]);
}
