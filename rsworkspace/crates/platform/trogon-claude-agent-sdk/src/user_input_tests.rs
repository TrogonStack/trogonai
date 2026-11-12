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
fn structured_input_keeps_content_and_identity() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = UserInput::blocks(vec![
        InputBlock::text("inspect"),
        InputBlock::from_json(serde_json::from_value(
            json!({"type":"image","source":{"type":"url","url":"https://example.com/image.png"}}),
        )?),
    ]);
    input.uuid = Some(MessageId::from("request-1"));
    let wire = input.to_wire_json()?;
    assert_eq!(wire["message"]["content"][1]["type"], "image");
    assert_eq!(wire["uuid"], "request-1");
    Ok(())
}
