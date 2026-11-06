use super::*;
use crate::fixtures::name;

#[test]
fn only_a_break_glass_actor_is_break_glass() {
    assert!(
        Actor::BreakGlass {
            operator: name("op"),
            host: name("host-1")
        }
        .is_break_glass()
    );
    assert!(!Actor::Unattributed.is_break_glass());
    assert!(
        !Actor::ApiKey {
            api_key_id: name("key")
        }
        .is_break_glass()
    );
    assert!(
        !Actor::HumanSession {
            subject: name("u"),
            issuer: name("i")
        }
        .is_break_glass()
    );
    assert!(!Actor::Agent { agent_id: name("a") }.is_break_glass());
}
