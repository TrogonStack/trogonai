use std::error::Error;

use crate::template::{TemplatePhase, TemplateRecord};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn a_record_round_trips_and_omits_an_absent_message() -> TestResult {
    let record = TemplateRecord {
        atespace: "tenant-a".into(),
        name: "tpl-1".into(),
        phase: TemplatePhase::Building,
        created_at: 7,
        message: None,
    };
    let json = serde_json::to_string(&record)?;
    assert!(!json.contains("message"), "{json}");
    assert!(json.contains("\"phase\":\"building\""), "{json}");
    let decoded: TemplateRecord = serde_json::from_str(&json)?;
    assert_eq!(decoded.phase, TemplatePhase::Building);
    assert_eq!(decoded.created_at, 7);
    Ok(())
}

#[test]
fn a_record_without_a_message_field_still_decodes() -> TestResult {
    let decoded: TemplateRecord =
        serde_json::from_str(r#"{"atespace":"a","name":"n","phase":"failed","created_at":1}"#)?;
    assert_eq!(decoded.phase, TemplatePhase::Failed);
    assert_eq!(decoded.message, None);
    Ok(())
}
