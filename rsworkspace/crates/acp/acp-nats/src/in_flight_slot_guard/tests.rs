use super::*;

fn counter() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}

fn read(counter: &Arc<AtomicUsize>) -> usize {
    counter.load(Ordering::Acquire)
}

#[test]
fn guard_increments_on_creation_decrements_on_drop() {
    let counter = counter();
    assert_eq!(read(&counter), 0);

    let guard = InFlightSlotGuard::new(counter.clone());
    assert_eq!(read(&counter), 1);

    drop(guard);
    assert_eq!(read(&counter), 0);
}

#[test]
fn multiple_guards_track_independently() {
    let counter = counter();
    let g1 = InFlightSlotGuard::new(counter.clone());
    let g2 = InFlightSlotGuard::new(counter.clone());
    assert_eq!(read(&counter), 2);

    drop(g1);
    assert_eq!(read(&counter), 1);

    drop(g2);
    assert_eq!(read(&counter), 0);
}

#[test]
fn saturating_sub_avoids_underflow() {
    let counter = counter();
    let guard = InFlightSlotGuard::new(counter.clone());
    counter.store(0, Ordering::Release);
    drop(guard);
    assert_eq!(read(&counter), 0);
}

/// The `Cell` version could not be shared across threads at all, so concurrent
/// release was untestable. Now that slots are released from arbitrary worker
/// threads, assert the counter actually returns to zero under contention.
#[test]
fn concurrent_guards_release_every_slot() {
    let counter = counter();
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let counter = counter.clone();
            std::thread::spawn(move || {
                for _ in 0..1_000 {
                    let _guard = InFlightSlotGuard::new(counter.clone());
                }
            })
        })
        .collect();

    for thread in threads {
        thread.join().expect("worker thread panicked");
    }

    assert_eq!(read(&counter), 0);
}
