use super::*;

#[test]
fn bucket_name_constant() {
    assert_eq!(A2A_AGENT_CARDS, "A2A_AGENT_CARDS");
}

#[test]
fn config_has_history_one() {
    let cfg = catalog_bucket_config();
    assert_eq!(cfg.history, 1);
}

#[test]
fn config_max_value_size_covers_typical_card() {
    let cfg = catalog_bucket_config();
    assert!(cfg.max_value_size >= 65536);
}
