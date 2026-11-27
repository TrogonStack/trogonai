use std::net::{Ipv4Addr, TcpListener};

use trogon_sandbox_windows_fence::{DEFAULT_PROXY_PORT_RANGE, ProxyPortRange, ProxyPortRangeError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn parses_and_displays() -> TestResult {
    assert_eq!("60080-60089".parse::<ProxyPortRange>()?, DEFAULT_PROXY_PORT_RANGE);
    assert_eq!(DEFAULT_PROXY_PORT_RANGE.to_string(), "60080-60089");
    assert_eq!(" 1 - 1 ".parse::<ProxyPortRange>()?, ProxyPortRange::new(1, 1)?);
    assert_eq!("1-65".parse::<ProxyPortRange>()?.high(), 65);
    assert_eq!(ProxyPortRange::default(), DEFAULT_PROXY_PORT_RANGE);
    assert_eq!(DEFAULT_PROXY_PORT_RANGE.low(), 60080);
    Ok(())
}

#[test]
fn rejects_what_the_helper_rejects() {
    for bad in ["60089-60080", "1-1000", "60080", "a-b", "0-65536", "0-9"] {
        assert!(bad.parse::<ProxyPortRange>().is_err(), "{bad}");
    }
}

#[test]
fn each_rejection_has_its_variant_and_text() {
    assert_eq!(ProxyPortRange::new(0, 5), Err(ProxyPortRangeError::ZeroLow));
    assert_eq!(
        ProxyPortRange::new(9, 5),
        Err(ProxyPortRangeError::Inverted { low: 9, high: 5 })
    );
    assert_eq!(
        ProxyPortRange::new(1, 1000),
        Err(ProxyPortRangeError::TooWide { ports: 1000, max: 65 })
    );
    assert_eq!(
        "60080".parse::<ProxyPortRange>(),
        Err(ProxyPortRangeError::MissingSeparator)
    );
    assert_eq!(
        "a-5".parse::<ProxyPortRange>(),
        Err(ProxyPortRangeError::InvalidLow("a".to_string()))
    );
    assert_eq!(
        "5-b".parse::<ProxyPortRange>(),
        Err(ProxyPortRangeError::InvalidHigh("b".to_string()))
    );
    assert_eq!(ProxyPortRangeError::ZeroLow.to_string(), "low port must be >= 1");
    assert_eq!(
        ProxyPortRangeError::Inverted { low: 9, high: 5 }.to_string(),
        "low (9) > high (5)"
    );
    assert_eq!(
        ProxyPortRangeError::TooWide { ports: 1000, max: 65 }.to_string(),
        "range too wide (1000 ports); max width 65"
    );
    assert_eq!(
        ProxyPortRangeError::MissingSeparator.to_string(),
        "expected LOW-HIGH (e.g. 60080-60089)"
    );
    assert_eq!(
        ProxyPortRangeError::InvalidLow("a".to_string()).to_string(),
        "invalid low port 'a'"
    );
    assert_eq!(
        ProxyPortRangeError::InvalidHigh("b".to_string()).to_string(),
        "invalid high port 'b'"
    );
}

fn held_port() -> Result<TcpListener, Box<dyn std::error::Error>> {
    loop {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let the_rest_binds =
            port < 65000 && (port + 1..=port + 8).all(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok());
        if the_rest_binds {
            return Ok(listener);
        }
    }
}

fn range_of(low: u16, width: u16) -> Result<ProxyPortRange, ProxyPortRangeError> {
    ProxyPortRange::new(low, low + width)
}

#[test]
fn listen_moves_past_a_port_in_use_and_holds_the_one_it_takes() -> TestResult {
    let blocker = held_port()?;
    let taken = blocker.local_addr()?.port();
    let range = range_of(taken, 8)?;
    let listener = range.listen(&[])?;
    let port = listener.local_addr()?.port();
    assert_ne!(port, taken);
    assert!(range.contains(port));
    assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_err());
    drop(blocker);
    Ok(())
}

#[test]
fn listen_skips_an_excluded_port() -> TestResult {
    let first = held_port()?.local_addr()?.port();
    let listener = range_of(first, 8)?.listen(&[first])?;
    assert_ne!(listener.local_addr()?.port(), first);
    Ok(())
}

#[test]
fn listen_reports_a_range_with_no_free_port() -> TestResult {
    let blocker = held_port()?;
    let taken = blocker.local_addr()?.port();
    let Err(error) = ProxyPortRange::new(taken, taken)?.listen(&[]) else {
        return Err("expected an error".into());
    };
    assert_eq!(
        error.to_string(),
        format!("No free port in range {taken}-{taken} (excluding )")
    );
    Ok(())
}

#[test]
fn contains_is_inclusive() {
    let r = DEFAULT_PROXY_PORT_RANGE;
    assert!(r.contains(60080) && r.contains(60089));
    assert!(!r.contains(60079) && !r.contains(60090));
}
