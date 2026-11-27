use super::*;
use std::time::UNIX_EPOCH;

#[test]
fn a_refusal_that_ends_unanswered_writes_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let request = AnsweredRequest::new("GET", false);
    assert!(RefusalError::Upgrade.answer(request, UNIX_EPOCH).is_empty());
    assert!(RefusalError::Upgrade.ends_unanswered());
    Ok(())
}
