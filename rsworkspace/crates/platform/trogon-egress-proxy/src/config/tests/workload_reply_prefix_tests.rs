use super::*;
use trogon_nats::DottedNatsToken;

#[test]
fn workload_reply_inboxes_are_typed_and_bound_to_the_registration_bucket() {
    for bucket in ["WORKLOAD_REGISTRATIONS".to_owned(), "B".repeat(64)] {
        let mut arguments = WORKLOAD_ARGS;
        let at = arguments
            .iter()
            .position(|value| *value == "--registration-bucket")
            .expect("bucket argument");
        arguments[at + 1] = &bucket;
        let config = workload_config(&arguments).expect("bounded workload configuration");
        let TlsInput::Workload(workload) = config.tls else {
            panic!("a Workload API configuration cannot become the file profile");
        };
        let prefix: &DottedNatsToken = workload.reply_inbox_prefix();
        assert_eq!(prefix.as_str(), format!("_INBOX.egress.{bucket}"));
        assert!(prefix.as_str().len() <= 128);
    }
}
