use super::*;

#[test]
fn says_why_the_state_could_not_be_read() {
    let unavailable = CredentialStateReadError::Unavailable {
        message: "no responders".to_owned(),
    };
    let undecodable = CredentialStateReadError::Undecodable {
        message: "unknown event".to_owned(),
    };

    assert_eq!(
        unavailable.to_string(),
        "credential stream is unavailable: no responders"
    );
    assert_eq!(
        undecodable.to_string(),
        "credential stream does not fold: unknown event"
    );
}
