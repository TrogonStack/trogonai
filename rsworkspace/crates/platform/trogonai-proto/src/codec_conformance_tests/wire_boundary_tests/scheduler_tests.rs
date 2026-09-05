use super::{assert_embedded_validation, assert_field_wire_types};
use crate::google::r#type::{DateTime, TimeZone};

#[test]
fn datetime_fields_validate_scalars_and_both_nested_offset_alternatives() {
    assert_field_wire_types::<DateTime>(&[8, 9], &[1, 2, 3, 4, 5, 6, 7]);
    assert_embedded_validation::<DateTime>(&[8, 9]);
    assert_field_wire_types::<TimeZone>(&[1, 2], &[]);
}
