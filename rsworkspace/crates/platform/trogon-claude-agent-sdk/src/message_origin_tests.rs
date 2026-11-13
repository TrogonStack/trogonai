#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use super::*;
use serde_json::json;

#[test]
fn extensions_cannot_override_origin_kind() -> Result<(), Box<dyn std::error::Error>> {
    let fields: JsonObject = serde_json::from_value(json!({"kind":"task-notification","source":"caller"}))?;
    let origin = MessageOrigin::new("human", fields.clone());
    assert_eq!(
        serde_json::to_value(&origin)?,
        json!({"kind":"human","source":"caller"})
    );
    assert!(matches!(
        MessageOrigin::validated("human", fields),
        Err(MessageOriginError::ReservedField)
    ));
    let origin: MessageOrigin = serde_json::from_value(json!({"kind":"human","source":"caller"}))?;
    assert_eq!(serde_json::to_value(origin)?["kind"], "human");
    assert_eq!(serde_json::from_value::<MessageOrigin>(json!({"kind":""}))?.kind(), "");
    Ok(())
}
