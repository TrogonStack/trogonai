use super::*;

#[test]
fn a_take_past_the_cap_is_refused() {
    let budget = ByteBudget::new(10);
    let held = budget.take(6);
    assert!(held.is_some());
    assert!(budget.take(5).is_none());
    assert!(budget.take(4).is_some());
}

#[test]
fn releasing_a_take_refills_the_budget() {
    let budget = ByteBudget::new(10);
    let held = budget.take(10);
    assert!(held.is_some());
    assert!(budget.take(1).is_none());
    drop(held);
    assert!(budget.take(10).is_some());
}

#[test]
fn a_take_larger_than_any_budget_is_refused() {
    let budget = ByteBudget::new(usize::MAX);
    assert!(budget.take(usize::MAX).is_none());
}
