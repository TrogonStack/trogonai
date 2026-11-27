use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use std::time::Duration;

use super::*;

fn endpoint(ip: &str) -> Result<VettedEndpoint, Box<dyn std::error::Error>> {
    Ok(crate::resolve::VettedAddr::assume_vetted(ip.parse::<IpAddr>()?).with_port(443))
}

fn spelled(endpoints: &[VettedEndpoint]) -> Vec<String> {
    endpoints.iter().map(|e| e.socket_addr().ip().to_string()).collect()
}

#[test]
fn families_alternate_starting_with_the_family_of_the_first_answer() -> Result<(), Box<dyn std::error::Error>> {
    let four_then_six = [
        endpoint("192.0.2.1")?,
        endpoint("192.0.2.2")?,
        endpoint("2001:db8::1")?,
        endpoint("2001:db8::2")?,
    ];
    assert_eq!(
        spelled(&in_wire_order(&four_then_six)),
        ["192.0.2.1", "2001:db8::1", "192.0.2.2", "2001:db8::2"]
    );
    let six_then_four = [
        endpoint("2001:db8::1")?,
        endpoint("2001:db8::2")?,
        endpoint("192.0.2.1")?,
        endpoint("192.0.2.2")?,
    ];
    assert_eq!(
        spelled(&in_wire_order(&six_then_four)),
        ["2001:db8::1", "192.0.2.1", "2001:db8::2", "192.0.2.2"]
    );
    let uneven = [
        endpoint("192.0.2.1")?,
        endpoint("192.0.2.2")?,
        endpoint("192.0.2.3")?,
        endpoint("2001:db8::1")?,
    ];
    assert_eq!(
        spelled(&in_wire_order(&uneven)),
        ["192.0.2.1", "2001:db8::1", "192.0.2.2", "192.0.2.3"]
    );
    Ok(())
}

fn log() -> Arc<Mutex<Vec<String>>> {
    Arc::default()
}

#[tokio::test(start_paused = true)]
async fn a_failed_attempt_hands_over_at_once() -> Result<(), Box<dyn std::error::Error>> {
    let tried = log();
    let seen = Arc::clone(&tried);
    let started = tokio::time::Instant::now();
    let won = first_to_connect(&[endpoint("192.0.2.1")?, endpoint("192.0.2.2")?], |e| {
        let seen = Arc::clone(&seen);
        async move {
            let ip = e.socket_addr().ip().to_string();
            seen.lock().map_err(|_| io::Error::other("poisoned"))?.push(ip.clone());
            if ip == "192.0.2.1" {
                Err(io::ErrorKind::ConnectionRefused.into())
            } else {
                Ok(ip)
            }
        }
    })
    .await?;
    assert_eq!(won, "192.0.2.2");
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_eq!(*tried.lock().map_err(|_| "poisoned")?, ["192.0.2.1", "192.0.2.2"]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_silent_attempt_is_given_250_milliseconds() -> Result<(), Box<dyn std::error::Error>> {
    let started = tokio::time::Instant::now();
    let won = first_to_connect(&[endpoint("192.0.2.1")?, endpoint("192.0.2.2")?], |e| async move {
        let ip = e.socket_addr().ip().to_string();
        if ip == "192.0.2.1" {
            std::future::pending::<()>().await;
        }
        Ok(ip)
    })
    .await?;
    assert_eq!(won, "192.0.2.2");
    assert_eq!(started.elapsed(), ATTEMPT_DELAY);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_last_attempt_has_no_limit_but_the_dial_ends_after_30_seconds() -> Result<(), Box<dyn std::error::Error>> {
    let started = tokio::time::Instant::now();
    let err = first_to_connect(&[endpoint("192.0.2.1")?, endpoint("192.0.2.2")?], |_| {
        std::future::pending::<io::Result<()>>()
    })
    .await
    .err()
    .ok_or("expected an error")?;
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    assert_eq!(started.elapsed(), DIAL_TIMEOUT);
    let slow = first_to_connect(&[endpoint("192.0.2.1")?], |_| async {
        tokio::time::sleep(Duration::from_secs(20)).await;
        Ok(())
    })
    .await;
    assert!(slow.is_ok());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn when_every_attempt_fails_the_last_error_is_reported() -> Result<(), Box<dyn std::error::Error>> {
    let err = first_to_connect(&[endpoint("192.0.2.1")?, endpoint("2001:db8::1")?], |e| async move {
        Err::<(), _>(io::Error::other(e.socket_addr().ip().to_string()))
    })
    .await
    .err()
    .ok_or("expected an error")?;
    assert_eq!(err.to_string(), "2001:db8::1");
    assert_eq!(
        first_to_connect::<(), _, _>(&[], |_| async { Ok(()) })
            .await
            .err()
            .ok_or("expected an error")?
            .kind(),
        io::ErrorKind::NotFound
    );
    Ok(())
}
