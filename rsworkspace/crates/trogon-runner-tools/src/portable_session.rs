use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortableMessage {
    pub role: String, // "user" | "assistant"
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::PortableMessage;

    #[test]
    fn portable_message_serde_round_trip() {
        let original = PortableMessage {
            role: "user".into(),
            text: "hello world".into(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: PortableMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.role, original.role);
        assert_eq!(decoded.text, original.text);
    }

    #[test]
    fn portable_message_vec_round_trip() {
        let original = vec![
            PortableMessage { role: "user".into(), text: "q".into() },
            PortableMessage { role: "assistant".into(), text: "a".into() },
        ];
        let json = serde_json::to_string(&original).unwrap();
        let decoded: Vec<PortableMessage> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].role, "user");
        assert_eq!(decoded[0].text, "q");
        assert_eq!(decoded[1].role, "assistant");
        assert_eq!(decoded[1].text, "a");
    }

    #[test]
    fn portable_message_json_shape() {
        let msg = PortableMessage { role: "user".into(), text: "hi".into() };
        let json = serde_json::to_string(&msg).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["role"], "user");
        assert_eq!(value["text"], "hi");
    }

    #[test]
    fn cross_runner_export_json_importable_by_all_runners() {
        let json = r#"[{"role":"user","text":"question"},{"role":"assistant","text":"answer"}]"#;
        let decoded: Vec<PortableMessage> = serde_json::from_str(json).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].role, "user");
        assert_eq!(decoded[0].text, "question");
        assert_eq!(decoded[1].role, "assistant");
        assert_eq!(decoded[1].text, "answer");
    }
}
