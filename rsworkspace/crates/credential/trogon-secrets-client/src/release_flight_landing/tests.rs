use std::future::pending;
use std::sync::{Arc, Mutex, PoisonError};

use super::ReleaseFlightLanding;
use crate::fixtures::{request, tenant};
use crate::{ReleaseCacheKey, ReleaseFlights};

fn on_board(flights: &Mutex<ReleaseFlights>, key: &ReleaseCacheKey) -> Option<u64> {
    flights
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(key)
        .map(|flight| flight.id())
}

fn boarded(flights: &Mutex<ReleaseFlights>, key: &ReleaseCacheKey, ticket: u64) -> u64 {
    flights.lock().unwrap().board(key.clone(), ticket, |_| pending()).id()
}

#[tokio::test]
async fn takes_its_call_off_the_board_when_dropped() {
    let flights = Arc::new(Mutex::new(ReleaseFlights::default()));
    let key = ReleaseCacheKey::of(&request(&tenant()));
    let id = boarded(&flights, &key, 0);
    let landing = ReleaseFlightLanding::new(Arc::clone(&flights), key.clone(), id);

    drop(landing);

    assert_eq!(on_board(&flights, &key), None);
}

#[tokio::test]
async fn leaves_a_call_that_took_its_place_on_the_board() {
    let flights = Arc::new(Mutex::new(ReleaseFlights::default()));
    let key = ReleaseCacheKey::of(&request(&tenant()));
    let earlier = boarded(&flights, &key, 0);
    let landing = ReleaseFlightLanding::new(Arc::clone(&flights), key.clone(), earlier);
    let later = boarded(&flights, &key, 1);

    drop(landing);

    assert_eq!(on_board(&flights, &key), Some(later));
}

#[tokio::test]
async fn lands_on_a_board_a_panic_left_poisoned() {
    let flights = Arc::new(Mutex::new(ReleaseFlights::default()));
    let key = ReleaseCacheKey::of(&request(&tenant()));
    let id = boarded(&flights, &key, 0);
    let poisoner = Arc::clone(&flights);
    let poisoned = std::thread::spawn(move || {
        let _held = poisoner.lock().unwrap();
        panic!("poisons the board");
    })
    .join();
    assert!(poisoned.is_err());
    assert!(flights.is_poisoned());
    let landing = ReleaseFlightLanding::new(Arc::clone(&flights), key.clone(), id);

    drop(landing);

    assert_eq!(on_board(&flights, &key), None);
}
