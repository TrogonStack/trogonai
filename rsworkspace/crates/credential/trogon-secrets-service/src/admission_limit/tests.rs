use tokio::sync::Semaphore;

use super::*;

#[test]
fn a_limit_of_zero_is_refused() {
    assert_eq!(AdmissionLimit::new(0), Err(AdmissionLimitError::Zero));
}

#[test]
fn a_limit_beyond_the_permit_range_is_refused() {
    assert_eq!(
        AdmissionLimit::new(Semaphore::MAX_PERMITS + 1),
        Err(AdmissionLimitError::TooLarge)
    );
}

#[test]
fn a_limit_keeps_its_value_up_to_the_permit_range() {
    assert_eq!(AdmissionLimit::new(1).unwrap().get(), 1);
    assert_eq!(
        AdmissionLimit::new(Semaphore::MAX_PERMITS).unwrap().get(),
        Semaphore::MAX_PERMITS
    );
}

#[test]
fn the_default_limit_is_the_service_default() {
    assert_eq!(AdmissionLimit::default().get(), DEFAULT_ADMISSION_LIMIT.get());
}
