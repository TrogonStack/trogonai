use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

struct Resource(Arc<AtomicUsize>);

impl Drop for Resource {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn lease() -> LeaseId {
    LeaseId::new("lease")
}

#[test]
fn a_borrowed_native_handle_stays_owned_until_cleanup_can_take_it() -> TestResult {
    let dropped = Arc::new(AtomicUsize::new(0));
    let retained = RetainedHandles::default();
    retained.insert(&lease(), Resource(dropped.clone()))?;
    let borrowed = retained.get(&lease()).ok_or("no handle")?;
    let first = retained.take_for_cleanup(&lease());
    assert!(matches!(first, Err(HandleStoreError::Borrowed)));
    assert_eq!(retained.len(), 1);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    drop(borrowed);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    let retried = retained.take_for_cleanup(&lease())?;
    assert!(retried.is_some());
    drop(retried);
    assert_eq!(retained.len(), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn cleanup_receipt_is_confirmed_only_after_the_resource_is_disposed() -> TestResult {
    let dropped = Arc::new(AtomicUsize::new(0));
    let retained = RetainedHandles::default();
    retained.insert(&lease(), Resource(dropped.clone()))?;
    let (id, resource) = retained.inventory().remove(0);
    let cleanup = retained.take_for_cleanup(&id)?.ok_or("no cleanup")?;
    assert_eq!(retained.len(), 1);
    assert!(!retained.is_completed(&id, &resource));
    assert!(retained.get(&id).is_none());
    cleanup.dispose(drop);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert!(retained.is_completed(&id, &resource));
    retained.insert(&lease(), Resource(dropped.clone()))?;
    assert!(!retained.matches(&id, &resource));
    assert!(retained.is_completed(&id, &resource));
    drop(retained.take_for_cleanup(&id)?);
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn a_second_cleanup_while_one_is_pending_is_refused() -> TestResult {
    let retained = RetainedHandles::default();
    retained.insert(&lease(), 1_u8)?;
    let pending = retained.take_for_cleanup(&lease())?;
    let second = retained.take_for_cleanup(&lease());
    assert!(matches!(second, Err(HandleStoreError::CleanupUnconfirmed)));
    drop(pending);
    assert!(retained.take_for_cleanup(&lease())?.is_none());
    Ok(())
}

#[test]
fn a_lease_cannot_retain_two_handles() -> TestResult {
    let retained = RetainedHandles::default();
    retained.insert(&lease(), 1_u8)?;
    assert!(retained.contains(&lease()));
    assert!(matches!(
        retained.insert(&lease(), 2_u8),
        Err(HandleStoreError::Retained)
    ));
    Ok(())
}

#[test]
fn a_poisoned_handle_is_still_cleaned_up() -> TestResult {
    let dropped = Arc::new(AtomicUsize::new(0));
    let retained = RetainedHandles::default();
    retained.insert(&lease(), Resource(dropped.clone()))?;
    let handle = retained.get(&lease()).ok_or("no handle")?;
    let poisoner = Arc::clone(&handle);
    let _ = std::thread::spawn(move || {
        let _held = poisoner.lock();
        std::panic::resume_unwind(Box::new("poisoned"));
    })
    .join();
    drop(handle);
    drop(retained.take_for_cleanup(&lease())?);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn a_poisoned_store_keeps_working() -> TestResult {
    let retained = Arc::new(RetainedHandles::default());
    let poisoner = Arc::clone(&retained);
    let _ = std::thread::spawn(move || {
        let _held = poisoner.0.entries.lock();
        std::panic::resume_unwind(Box::new("poisoned"));
    })
    .join();
    retained.insert(&lease(), 1_u8)?;
    assert_eq!(retained.len(), 1);
    Ok(())
}
