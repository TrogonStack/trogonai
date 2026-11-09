use super::super::constants::{
    CLIENT_PING_INTERVAL, CLIENT_PING_TIMEOUT, SERVER_PING_TIMEOUT, SOCKET_LIVENESS, UNANSWERED_PING_LIMIT,
};
use super::*;

#[test]
fn default_liveness_mirrors_the_sdk_timeouts() {
    let liveness = SocketLiveness::new(
        CLIENT_PING_INTERVAL,
        CLIENT_PING_TIMEOUT,
        SERVER_PING_TIMEOUT,
        UNANSWERED_PING_LIMIT,
    );

    assert_eq!(liveness, SOCKET_LIVENESS);
    assert_eq!(liveness.client_ping_timeout(), Duration::from_secs(5));
    assert_eq!(liveness.server_ping_timeout(), Duration::from_secs(30));
    assert_eq!(liveness.ping_interval(), Duration::from_secs(5) / 3);
}

#[tokio::test(start_paused = true)]
async fn pings_that_never_get_a_pong_stall_once_the_limit_of_intervals_passes() {
    let mut watch = PongWatch::new(SOCKET_LIVENESS);
    let first = Instant::now();

    watch.ping_sent(first);
    watch.ping_sent(first + CLIENT_PING_INTERVAL);

    assert_eq!(
        watch.stall_deadline(),
        Some(first + CLIENT_PING_INTERVAL * UNANSWERED_PING_LIMIT)
    );
}

#[tokio::test(start_paused = true)]
async fn an_unanswered_ping_stalls_the_connection_a_timeout_after_it_was_sent() {
    let mut watch = PongWatch::new(SOCKET_LIVENESS);
    let sent = Instant::now();
    watch.pong_received();

    assert_eq!(watch.stall_deadline(), None);
    watch.ping_sent(sent);
    assert_eq!(watch.stall_deadline(), Some(sent + CLIENT_PING_TIMEOUT));
}

#[tokio::test(start_paused = true)]
async fn a_later_ping_does_not_postpone_the_stall_deadline() {
    let mut watch = PongWatch::new(SOCKET_LIVENESS);
    let first = Instant::now();
    watch.pong_received();
    watch.ping_sent(first);

    watch.ping_sent(first + Duration::from_secs(1));

    assert_eq!(watch.stall_deadline(), Some(first + CLIENT_PING_TIMEOUT));
}

#[tokio::test(start_paused = true)]
async fn a_pong_clears_the_stall_deadline_however_late_it_is_read() {
    let mut watch = PongWatch::new(SOCKET_LIVENESS);
    let sent = Instant::now();
    watch.pong_received();
    watch.ping_sent(sent);

    tokio::time::advance(CLIENT_PING_TIMEOUT * 4).await;
    watch.pong_received();

    assert_eq!(watch.stall_deadline(), None);
}

#[tokio::test(start_paused = true)]
async fn there_is_no_stall_deadline_before_a_ping_is_sent() {
    let watch = PongWatch::new(SOCKET_LIVENESS);

    assert_eq!(watch.stall_deadline(), None);
}

#[tokio::test(start_paused = true)]
async fn a_first_pong_switches_to_the_ping_timeout() {
    let mut watch = PongWatch::new(SOCKET_LIVENESS);
    let first = Instant::now();
    watch.ping_sent(first);

    watch.pong_received();
    let later = first + CLIENT_PING_INTERVAL;
    watch.ping_sent(later);

    assert_eq!(watch.stall_deadline(), Some(later + CLIENT_PING_TIMEOUT));
}
