use super::*;

#[test]
fn the_pool_hands_back_what_it_kept_and_drops_the_overflow() {
    let mut pool = WarmPool::new(1);
    pool.give(1);
    pool.give(2);
    assert_eq!(pool.take(), Some(1));
    assert_eq!(pool.take(), None);
}

#[test]
fn a_pool_with_no_capacity_keeps_nothing() {
    let mut pool = WarmPool::new(0);
    pool.give(1);
    assert_eq!(pool.take(), None);
}
