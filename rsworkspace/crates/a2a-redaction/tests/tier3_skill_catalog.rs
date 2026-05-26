use a2a_redaction::{Redactor, SkillId, WasmBundlePath, WasmRedactorHost};
use a2a_types::{part, Message, Part, Role};

fn bundled_wasm(name: &str) -> &'static [u8] {
    match name {
        "pii_regex_redactor" => include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../a2a-pack/skills/pii_regex_redactor.wasm"
        )),
        "secrets_redactor" => include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../a2a-pack/skills/secrets_redactor.wasm"
        )),
        "json_path_sanitizer" => include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../a2a-pack/skills/json_path_sanitizer.wasm"
        )),
        other => panic!("unknown bundled wasm fixture: {other}"),
    }
}

fn host_with_skill(skill_slug: &str, wasm_fixture: &str) -> WasmRedactorHost {
    let host = WasmRedactorHost::new(WasmBundlePath::new(std::env::temp_dir())).expect("host");
    host.register_skill_wasm(SkillId::new(skill_slug), bundled_wasm(wasm_fixture))
        .expect("register wasm");
    host
}

fn text_part(text: &str) -> Part {
    Part {
        content: Some(part::Content::Text(text.into())),
        ..Default::default()
    }
}

#[test]
fn pii_regex_redactor_wasm_masks_email_in_message_part() {
    let host = host_with_skill("pii-regex-redactor", "pii_regex_redactor");
    let msg_in = Message {
        message_id: "m1".into(),
        role: Role::User.into(),
        parts: vec![text_part("contact alice@example.com today")],
        ..Default::default()
    };
    let msg_out = host
        .redact_message(msg_in, &SkillId::new("pii-regex-redactor"))
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
fn secrets_redactor_wasm_masks_github_pat_in_message_part() {
    let host = host_with_skill("secrets-redactor", "secrets_redactor");
    let msg_in = Message {
        message_id: "m2".into(),
        role: Role::Agent.into(),
        parts: vec![text_part("pat ghp_1234567890abcdefghijklmnopqrstuvwxyz")],
        ..Default::default()
    };
    let msg_out = host
        .redact_message(msg_in, &SkillId::new("secrets-redactor"))
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
        role: Role::User.into(),
        parts: vec![part],
        ..Default::default()
    };
    let msg_out = host
        .redact_message(msg_in, &SkillId::new("json-path-sanitizer"))
        .expect("redact");
    let part_json = serde_json::to_value(&msg_out.parts[0]).expect("part json");
    assert_eq!(part_json["metadata"]["credentials"].as_str(), Some("[REDACTED]"));
    assert_eq!(part_json["metadata"]["note"].as_str(), Some("visible"));
}
