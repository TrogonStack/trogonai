#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use super::*;

#[test]
fn interrupt_keeps_legacy_absence_distinct() -> Result<(), serde_json::Error> {
    assert!(matches!(
        InterruptReceipt::from_json(JsonObject::new()),
        InterruptReceipt::NotReported { .. }
    ));
    let value = serde_json::json!({"still_queued":[],"cancelled":[]});
    let value: JsonObject = serde_json::from_value(value)?;
    assert!(
        matches!(InterruptReceipt::from_json(value), InterruptReceipt::Reported { still_queued, .. } if still_queued.is_empty())
    );
    Ok(())
}
