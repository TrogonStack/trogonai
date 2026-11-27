use trogon_std::env::InMemoryEnv;

use super::*;

#[test]
fn no_key_in_the_environment_means_no_signer() {
    assert!(matches!(signer_from_env(&InMemoryEnv::new()), Ok(None)));
}
