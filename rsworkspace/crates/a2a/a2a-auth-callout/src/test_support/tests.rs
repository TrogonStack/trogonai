use std::time::Duration;

use super::*;

#[test]
fn minted_test_users_carry_the_requested_caller_and_audience() {
    let jwt = mint_test_user_jwt("coveragecaller", "ACCOUNT", Duration::from_secs(30));
    let payload = crate::jwt::decode_nats_user_payload(jwt.as_str()).expect("decode test user");

    assert_eq!(payload["caller_id"], "coveragecaller");
    assert_eq!(payload["aud"], "ACCOUNT");
}
