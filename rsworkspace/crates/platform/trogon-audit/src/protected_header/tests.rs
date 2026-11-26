use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::*;
use crate::fixtures::signing_key;
use crate::key_id::KeyId;

fn encoded(json: &str) -> String {
    URL_SAFE_NO_PAD.encode(json)
}

#[test]
fn reads_the_header_the_writer_writes() {
    let key = KeyId::from_verifying_key(signing_key(1).verifying_key());
    let header = ProtectedHeader::decode(&checkpoint_jws::protected_header(&key)).unwrap();

    assert_eq!(
        header,
        ProtectedHeader {
            alg: "EdDSA".to_owned(),
            kid: key.to_string(),
            typ: "trogonai-audit-checkpoint+jws".to_owned(),
        }
    );
}

#[test]
fn refuses_a_member_the_writer_never_sets() {
    let json = r#"{"alg":"EdDSA","kid":"k","typ":"t","b64":false,"crit":["b64"]}"#;
    assert_eq!(ProtectedHeader::decode(&encoded(json)), None);
}

#[test]
fn refuses_a_missing_member_and_what_is_not_json() {
    assert_eq!(ProtectedHeader::decode(&encoded(r#"{"alg":"EdDSA","kid":"k"}"#)), None);
    assert_eq!(ProtectedHeader::decode(&encoded("not json")), None);
    assert_eq!(ProtectedHeader::decode("not base64!"), None);
}
