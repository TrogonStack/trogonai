use super::*;

#[test]
fn guard_increments_on_creation_decrements_on_drop() {
    let counter = Rc::new(Cell::new(0usize));
    assert_eq!(counter.get(), 0);

    let guard = InFlightSlotGuard::new(counter.clone());
    assert_eq!(counter.get(), 1);

    drop(guard);
    assert_eq!(counter.get(), 0);
}

#[test]
fn multiple_guards_track_independently() {
    let counter = Rc::new(Cell::new(0usize));
    let g1 = InFlightSlotGuard::new(counter.clone());
    let g2 = InFlightSlotGuard::new(counter.clone());
    assert_eq!(counter.get(), 2);

    drop(g1);
    assert_eq!(counter.get(), 1);

    drop(g2);
    assert_eq!(counter.get(), 0);
}

#[test]
fn saturating_sub_avoids_underflow() {
    let counter = Rc::new(Cell::new(0usize));
    let guard = InFlightSlotGuard::new(counter.clone());
    counter.set(0);
    drop(guard);
    assert_eq!(counter.get(), 0);
}
