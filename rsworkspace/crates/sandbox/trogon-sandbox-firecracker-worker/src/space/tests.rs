type TestResult = Result<(), Box<dyn std::error::Error>>;
use super::*;

#[test]
fn writer_owners_keep_filesystem_capacity_until_every_owner_releases_it() -> TestResult {
    let ledger = Arc::new(Reservations::default());
    let available = FREE_MARGIN + 100;
    let first = ledger.reserve(7, 60, || Ok(available))?;
    assert!(ledger.reserve(7, 50, || Ok(available)).is_err());
    let writer = Arc::clone(&first);
    drop(first);
    assert!(ledger.reserve(7, 50, || Ok(available)).is_err());
    drop(writer);
    assert!(ledger.reserve(7, 50, || Ok(available)).is_ok());
    assert!(ledger.reserve(8, 100, || Ok(available)).is_ok());
    Ok(())
}

#[test]
fn contending_leases_cannot_both_reserve_the_same_free_capacity() -> TestResult {
    let ledger = Arc::new(Reservations::default());
    let start = Arc::new(std::sync::Barrier::new(3));
    let finished = Arc::new(std::sync::Barrier::new(3));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let ledger = Arc::clone(&ledger);
        let start = Arc::clone(&start);
        let finished = Arc::clone(&finished);
        threads.push(std::thread::spawn(move || {
            start.wait();
            let reservation = ledger.reserve(7, 60, || Ok(FREE_MARGIN + 100));
            finished.wait();
            reservation
        }));
    }
    start.wait();
    finished.wait();
    let held: Vec<_> = threads
        .into_iter()
        .map(|thread| {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("a reserving thread panicked"))
        })
        .collect::<anyhow::Result<_>>()?;
    assert_eq!(held.iter().filter(|result| result.is_ok()).count(), 1);
    Ok(())
}

#[tokio::test]
async fn cancelling_a_blocking_writer_keeps_its_capacity_until_the_write_finishes() -> TestResult {
    let ledger = Arc::new(Reservations::default());
    let reservation = ledger.reserve(7, 60, || Ok(FREE_MARGIN + 100))?;
    let weak = Arc::downgrade(&reservation);
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (finished, done) = tokio::sync::oneshot::channel();
    let gate = Arc::new(std::sync::Barrier::new(2));
    let writer_gate = Arc::clone(&gate);
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("output");
    let written = path.clone();
    let task = tokio::spawn(async move {
        let writer_reservation = Arc::clone(&reservation);
        let _ = tokio::task::spawn_blocking(move || {
            let _ = entered.send(());
            writer_gate.wait();
            let _ = std::fs::write(written, b"completed after cancellation");
            drop(writer_reservation);
            let _ = finished.send(());
        })
        .await;
        drop(reservation);
    });
    ready.await?;
    task.abort();
    assert!(task.await.err().ok_or("the task must be cancelled")?.is_cancelled());
    assert!(weak.upgrade().is_some());
    assert!(ledger.reserve(7, 60, || Ok(FREE_MARGIN + 100)).is_err());
    gate.wait();
    done.await?;
    assert!(weak.upgrade().is_none());
    assert_eq!(std::fs::read(path)?, b"completed after cancellation");
    assert!(ledger.reserve(7, 60, || Ok(FREE_MARGIN + 100)).is_ok());
    Ok(())
}
