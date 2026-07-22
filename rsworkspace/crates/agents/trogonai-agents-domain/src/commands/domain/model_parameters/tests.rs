use super::*;

#[test]
fn validates_ordered_parameters_but_keeps_values_opaque() {
    let params = ModelParameters::new(BTreeMap::from([
        ("temperature".to_string(), "0.2".to_string()),
        ("note".to_string(), " opaque value ".to_string()),
    ]))
    .unwrap();
    assert_eq!(params.as_map().keys().next().map(String::as_str), Some("note"));
    assert!(ModelParameters::new(BTreeMap::from([(String::new(), "x".to_string())])).is_err());
}
