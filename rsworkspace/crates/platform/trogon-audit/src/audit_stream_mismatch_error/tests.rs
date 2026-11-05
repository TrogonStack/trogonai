use super::*;

#[test]
fn each_message_names_the_rule_and_the_stream() {
    let stream = "AUDIT";
    let cases = [
        (
            AuditStreamMismatchError::Storage { stream },
            "stream 'AUDIT' must use file storage",
        ),
        (
            AuditStreamMismatchError::Retention { stream },
            "stream 'AUDIT' must use limits retention",
        ),
        (
            AuditStreamMismatchError::Discard { stream },
            "stream 'AUDIT' must refuse writes when full, not discard the oldest record",
        ),
        (
            AuditStreamMismatchError::DenyDelete { stream },
            "stream 'AUDIT' must deny deleting a record",
        ),
        (
            AuditStreamMismatchError::DenyPurge { stream },
            "stream 'AUDIT' must deny purging records",
        ),
        (
            AuditStreamMismatchError::NoAck { stream },
            "stream 'AUDIT' must acknowledge publishes",
        ),
        (
            AuditStreamMismatchError::MaxAge { stream },
            "stream 'AUDIT' must not expire records by age",
        ),
        (
            AuditStreamMismatchError::MessageTtl { stream },
            "stream 'AUDIT' must not let a record set its own expiry",
        ),
        (
            AuditStreamMismatchError::MaxMessagesPerSubject { stream },
            "stream 'AUDIT' must not limit the records kept per subject",
        ),
        (
            AuditStreamMismatchError::DuplicateWindow { stream },
            "stream 'AUDIT' must keep a duplicate window of two minutes or longer",
        ),
        (
            AuditStreamMismatchError::Replicas { stream },
            "stream 'AUDIT' must keep at least the replicas the deployment asked for",
        ),
        (
            AuditStreamMismatchError::MissingSubject {
                stream,
                subject: "platform.v1.audit.change.>".to_owned(),
            },
            "stream 'AUDIT' is missing a subject the audit kinds are published on: 'platform.v1.audit.change.>'",
        ),
    ];
    for (error, message) in cases {
        assert_eq!(error.to_string(), message);
    }
}
