use super::*;

#[test]
fn the_default_bounds_are_modest() {
    let config = WarmPoolConfig::default();
    assert_eq!(
        (
            config.max_pool_size_per_digest,
            config.max_total_warm,
            config.max_idle_seconds,
            config.refill_per_pass,
            config.max_warming
        ),
        (3, 16, 600, 2, 4)
    );
}
