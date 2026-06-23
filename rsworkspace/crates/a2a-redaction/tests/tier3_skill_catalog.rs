#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use a2a::types::{Message, Part, PartContent, Role};
use a2a_redaction::{Redactor, SkillId, WasmBundlePath, WasmRedactorHost};

// NOTE: The real Tier-3 skill wasms (pii_regex_redactor, secrets_redactor,
// json_path_sanitizer) live under a2a-pack/skills/ and haven't been
// extracted into main yet. Until those fixtures land, every test in this
// file is `#[ignore]`d and the loader falls back to the bundled identity
// fixture so the file at least compiles. Re-enable by removing the
// ignore attribute once the a2a-pack/skills/ wasm fixtures merge.
fn bundled_wasm(_name: &str) -> &'static [u8] {
    include_bytes!("fixtures/identity_redact_part.wasm")
}

fn host_with_skill(skill_slug: &str, wasm_fixture: &str) -> WasmRedactorHost {
    let host = WasmRedactorHost::new(WasmBundlePath::new(std::env::temp_dir())).expect("host");
    host.register_skill_wasm(SkillId::new(skill_slug).expect("valid"), bundled_wasm(wasm_fixture))
        .expect("register wasm");
    host
}

fn text_part(text: &str) -> Part {
    Part {
        content: PartContent::Text(text.into()),
        filename: None,
        media_type: None,
        metadata: None,
    }
}

#[test]
#[ignore = "depends on a2a-pack/skills/*.wasm fixtures (extracted in a later slice)"]
fn pii_regex_redactor_wasm_masks_email_in_message_part() {
    let host = host_with_skill("pii-regex-redactor", "pii_regex_redactor");
    let msg_in = Message {
        message_id: "m1".into(),
        context_id: None,
        task_id: None,
        role: Role::User,
        parts: vec![text_part("contact alice@example.com today")],
        metadata: None,
        extensions: None,
        reference_task_ids: None,
    };
    let msg_out = host
        .redact_message(msg_in, &SkillId::new("pii-regex-redactor").expect("valid"))
        .expect("redact");
    let part_json = serde_json::to_value(&msg_out.parts[0]).expect("part json");
    let text = part_json["content"]["text"]
        .as_str()
        .or_else(|| part_json["text"].as_str())
        .expect("text field");
    assert!(!text.contains("alice@example.com"));
    assert!(text.contains("[REDACTED]"));
}

#[test]
#[ignore = "depends on a2a-pack/skills/*.wasm fixtures (extracted in a later slice)"]
fn secrets_redactor_wasm_masks_github_pat_in_message_part() {
    let host = host_with_skill("secrets-redactor", "secrets_redactor");
    let msg_in = Message {
        message_id: "m2".into(),
        context_id: None,
        task_id: None,
        role: Role::Agent,
        parts: vec![text_part("pat ghp_1234567890abcdefghijklmnopqrstuvwxyz")],
        metadata: None,
        extensions: None,
        reference_task_ids: None,
    };
    let msg_out = host
        .redact_message(msg_in, &SkillId::new("secrets-redactor").expect("valid"))
        .expect("redact");
    let part_json = serde_json::to_value(&msg_out.parts[0]).expect("part json");
    let text = part_json["content"]["text"]
        .as_str()
        .or_else(|| part_json["text"].as_str())
        .expect("text field");
    assert!(!text.contains("ghp_"));
    assert!(text.contains("[REDACTED]"));
}

#[test]
#[ignore = "depends on a2a-pack/skills/*.wasm fixtures (extracted in a later slice)"]
fn json_path_sanitizer_wasm_redacts_denylisted_metadata_field() {
    let host = host_with_skill("json-path-sanitizer", "json_path_sanitizer");
    let part: Part = serde_json::from_value(serde_json::json!({
        "metadata": {
            "credentials": "top-secret",
            "note": "visible"
        }
    }))
    .expect("part json");
    let msg_in = Message {
        message_id: "m3".into(),
        context_id: None,
        task_id: None,
        role: Role::User,
        parts: vec![part],
        metadata: None,
        extensions: None,
        reference_task_ids: None,
    };
    let msg_out = host
        .redact_message(msg_in, &SkillId::new("json-path-sanitizer").expect("valid"))
        .expect("redact");
    let part_json = serde_json::to_value(&msg_out.parts[0]).expect("part json");
    assert_eq!(part_json["metadata"]["credentials"].as_str(), Some("[REDACTED]"));
    assert_eq!(part_json["metadata"]["note"].as_str(), Some("visible"));
}
