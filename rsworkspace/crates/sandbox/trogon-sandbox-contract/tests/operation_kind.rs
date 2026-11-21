use std::error::Error;

use trogon_sandbox_contract::OperationKind;

#[test]
fn every_kind_preserves_its_wire_spelling() -> Result<(), Box<dyn Error>> {
    let fixtures = [
        (OperationKind::Create, "create", r#""create""#),
        (OperationKind::Restore, "restore", r#""restore""#),
        (OperationKind::Fork, "fork", r#""fork""#),
        (OperationKind::Snapshot, "snapshot", r#""snapshot""#),
        (OperationKind::Pause, "pause", r#""pause""#),
        (OperationKind::Resume, "resume", r#""resume""#),
        (OperationKind::Suspend, "suspend", r#""suspend""#),
        (OperationKind::Wake, "wake", r#""wake""#),
        (OperationKind::Stop, "stop", r#""stop""#),
        (OperationKind::Delete, "delete", r#""delete""#),
        (OperationKind::Rollback, "rollback", r#""rollback""#),
    ];
    for (kind, name, json) in fixtures {
        assert_eq!(kind.as_str(), name);
        assert_eq!(serde_json::to_string(&kind)?, json);
        assert_eq!(serde_json::from_str::<OperationKind>(json)?, kind);
    }
    Ok(())
}

#[test]
fn unsupported_kind_spellings_are_rejected() {
    for json in [r#""Create""#, r#""CREATE""#, r#""unknown""#, "null", "0"] {
        assert!(serde_json::from_str::<OperationKind>(json).is_err(), "decoded {json}");
    }
}
