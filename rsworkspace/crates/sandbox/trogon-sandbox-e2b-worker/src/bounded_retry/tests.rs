use std::cell::Cell;

use super::bounded_retry;

#[derive(Debug, Eq, PartialEq)]
enum Refusal {
    Exhausted,
    Failed,
}

#[tokio::test]
async fn returns_the_first_value() -> Result<(), Refusal> {
    let calls = Cell::new(0);
    let value = bounded_retry(3, Refusal::Exhausted, || async {
        calls.set(calls.get() + 1);
        Ok(if calls.get() == 2 { Some(7) } else { None })
    })
    .await?;
    assert_eq!(value, 7);
    assert_eq!(calls.get(), 2);
    Ok(())
}

#[tokio::test]
async fn gives_up_after_the_bound() {
    let calls = Cell::new(0);
    let result: Result<u8, Refusal> = bounded_retry(3, Refusal::Exhausted, || async {
        calls.set(calls.get() + 1);
        Ok(None)
    })
    .await;
    assert_eq!(result, Err(Refusal::Exhausted));
    assert_eq!(calls.get(), 3);
}

#[tokio::test]
async fn stops_on_an_error() {
    let result: Result<u8, Refusal> = bounded_retry(3, Refusal::Exhausted, || async { Err(Refusal::Failed) }).await;
    assert_eq!(result, Err(Refusal::Failed));
}
