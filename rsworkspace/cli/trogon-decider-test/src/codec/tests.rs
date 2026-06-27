use super::*;

#[test]
fn normalize_adds_prefix() {
    assert_eq!(
        normalize_type_url("trogonai.example.light.v1.TurnOn"),
        "type.googleapis.com/trogonai.example.light.v1.TurnOn"
    );
}
