//! Integration tests for context-aware token-bucket throttling (no NATS).

use std::time::Instant;

use trogon_mcp_gateway::context_throttle::{
    ContextBudget, ContextThrottle, ContextThrottleConfig, ContextThrottleKey,
    ContextThrottleOutcome, TestClock,
};

#[test]
fn acquire_then_throttle_with_test_clock() {
    let clock = TestClock::new(Instant::now());
    let config = ContextThrottleConfig {
        default_budget: ContextBudget {
            tokens_per_minute: 60,
            burst: 2,
        },
        ..ContextThrottleConfig::default()
    };
    let throttle = ContextThrottle::with_clock(config, clock);
    let key =
        ContextThrottleKey::new("acme", "agent/e2e", "incident").expect("valid key");

    assert_eq!(
        throttle.acquire(&key, 1).expect("first"),
        ContextThrottleOutcome::Allowed
    );
    assert_eq!(
        throttle.acquire(&key, 1).expect("second"),
        ContextThrottleOutcome::Allowed
    );
    match throttle.acquire(&key, 1).expect("third") {
        ContextThrottleOutcome::Throttled { retry_after_ms } => {
            assert!(retry_after_ms >= 1);
        }
        ContextThrottleOutcome::Allowed => panic!("expected throttle after burst exhausted"),
    }
}
