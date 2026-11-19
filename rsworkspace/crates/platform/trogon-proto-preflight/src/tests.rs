use super::*;

#[test]
fn scalar_rejects_non_scalar_internal_dispatch_without_consuming_input() {
    let field = FieldShape {
        kind: "TYPE_BYTES".into(),
        type_name: None,
        repeated: false,
        required: false,
        oneof: None,
    };
    let mut offset = 0;
    assert_eq!(
        scalar(&SchemaData::default(), &field, 1, &[], &mut offset),
        Err(WirePreflightError::WrongWireType { field_number: 1 })
    );
    assert_eq!(offset, 0);
}
