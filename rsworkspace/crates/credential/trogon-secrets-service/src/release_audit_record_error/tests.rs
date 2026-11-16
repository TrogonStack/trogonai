use trogon_audit::AuditName;

use super::*;

#[test]
fn a_name_error_names_the_fact_it_could_not_carry() {
    let source = AuditName::new("use=1").unwrap_err();
    let reason = source.to_string();
    let error = ReleaseAuditRecordError::Name {
        field: "correlation id",
        source,
    };

    assert_eq!(
        error.to_string(),
        format!("correlation id is not an audit name: {reason}")
    );
}
