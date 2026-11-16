use std::future::pending;

use super::ReleaseFlights;
use crate::fixtures::{request, tenant};
use crate::{ReleaseCacheKey, ReleaseError};

fn key() -> ReleaseCacheKey {
    ReleaseCacheKey::of(&request(&tenant()))
}

#[tokio::test]
async fn has_no_call_for_a_key_nobody_asked_for() {
    let mut flights = ReleaseFlights::default();
    flights.board(key(), 0, |_| pending());

    assert!(flights.get(&key()).is_none());
}

#[tokio::test]
async fn keeps_a_call_on_the_board_with_the_ticket_it_was_asked_at() {
    let mut flights = ReleaseFlights::default();
    let key = key();

    let boarded = flights.board(key.clone(), 7, |_| pending());

    let flight = flights.get(&key).expect("the call is on its way");
    assert_eq!(flight.id(), boarded.id());
    assert_eq!(flight.ticket(), 7);
}

#[tokio::test]
async fn takes_a_call_off_the_board_when_it_lands() {
    let mut flights = ReleaseFlights::default();
    let key = key();
    let boarded = flights.board(key.clone(), 0, |_| pending());

    flights.land(&key, boarded.id());

    assert!(flights.get(&key).is_none());
}

#[tokio::test]
async fn keeps_a_later_call_on_the_board_when_an_earlier_one_lands() {
    let mut flights = ReleaseFlights::default();
    let key = key();
    let earlier = flights.board(key.clone(), 0, |_| pending());
    let later = flights.board(key.clone(), 1, |_| pending());

    flights.land(&key, earlier.id());

    let flight = flights.get(&key).expect("the later call is still on its way");
    assert_eq!(flight.id(), later.id());
}

#[tokio::test]
async fn hands_the_call_the_id_it_lands_with() {
    let mut flights = ReleaseFlights::default();
    let (handed, handed_rx) = tokio::sync::oneshot::channel();

    let boarded = flights.board(key(), 0, move |id| {
        handed.send(id).expect("the test waits for the id");
        pending()
    });

    assert_eq!(handed_rx.await.expect("the call was handed its id"), boarded.id());
}

#[tokio::test]
async fn gives_every_waiter_what_the_call_landed_with() {
    let mut flights = ReleaseFlights::default();
    let flight = flights.board(key(), 0, |_| async { Err(ReleaseError::Denied) });

    let (first, second) = tokio::join!(flight.clone().landed(), flight.landed());

    assert!(matches!(first, Err(ReleaseError::Denied)));
    assert!(matches!(second, Err(ReleaseError::Denied)));
}

#[tokio::test]
async fn fails_whoever_waits_on_a_call_that_broke_before_landing() {
    let mut flights = ReleaseFlights::default();
    let flight = flights.board(key(), 0, |_| async { panic!("the call broke") });

    let landed = flight.landed().await;

    assert!(matches!(landed, Err(ReleaseError::Request(_))));
}
