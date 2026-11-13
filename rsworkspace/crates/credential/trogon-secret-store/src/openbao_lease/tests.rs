use super::*;

fn token() -> SecretString {
    SecretString::new("fake-login-token").unwrap()
}

fn lease(at: Instant, ttl: Duration, renewal: OpenBaoLeaseRenewal) -> OpenBaoLease {
    OpenBaoLease::new(token(), at, ttl, renewal)
}

#[test]
fn a_token_inside_the_expiry_margin_is_not_usable() {
    let start = Instant::now();
    let lease = lease(start, Duration::from_secs(60), OpenBaoLeaseRenewal::Renewable);

    assert!(lease.is_usable(start + Duration::from_secs(60) - TOKEN_EXPIRY_MARGIN - Duration::from_millis(1)));
    assert!(!lease.is_usable(start + Duration::from_secs(60) - TOKEN_EXPIRY_MARGIN));
    assert!(!lease.is_usable(start + Duration::from_secs(61)));
}

#[test]
fn a_lease_shorter_than_the_margin_is_never_usable() {
    let start = Instant::now();
    let lease = lease(start, TOKEN_EXPIRY_MARGIN, OpenBaoLeaseRenewal::Renewable);

    assert!(!lease.is_usable(start));
}

#[test]
fn renewal_is_due_in_the_last_part_of_the_lease() {
    let start = Instant::now();
    let ttl = Duration::from_secs(60);
    let lease = lease(start, ttl, OpenBaoLeaseRenewal::Renewable);

    assert!(!lease.is_renewal_due(start));
    assert!(!lease.is_renewal_due(start + ttl - ttl / RENEWAL_WINDOW_DIVISOR - Duration::from_millis(1)));
    assert!(lease.is_renewal_due(start + ttl - ttl / RENEWAL_WINDOW_DIVISOR));
}

#[test]
fn renewal_is_due_no_later_than_the_expiry_margin_on_a_short_lease() {
    let start = Instant::now();
    let ttl = TOKEN_EXPIRY_MARGIN * 2;
    let lease = lease(start, ttl, OpenBaoLeaseRenewal::Renewable);

    assert!(lease.is_renewal_due(start + ttl - TOKEN_EXPIRY_MARGIN));
}

#[test]
fn a_fixed_lease_cannot_be_renewed() {
    let start = Instant::now();
    let lease = lease(start, Duration::from_secs(60), OpenBaoLeaseRenewal::Fixed);

    assert!(!lease.can_renew(start));
}

#[test]
fn an_expired_lease_cannot_be_renewed() {
    let start = Instant::now();
    let lease = lease(start, Duration::from_secs(60), OpenBaoLeaseRenewal::Renewable);

    assert!(lease.can_renew(start));
    assert!(!lease.can_renew(start + Duration::from_secs(60)));
}

#[test]
fn reports_the_renewal_flag_as_an_enum() {
    assert_eq!(OpenBaoLeaseRenewal::from(true), OpenBaoLeaseRenewal::Renewable);
    assert_eq!(OpenBaoLeaseRenewal::from(false), OpenBaoLeaseRenewal::Fixed);
}
