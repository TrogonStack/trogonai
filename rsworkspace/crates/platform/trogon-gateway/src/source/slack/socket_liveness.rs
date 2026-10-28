use std::time::Duration;

use tokio::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SocketLiveness {
    ping_interval: Duration,
    client_ping_timeout: Duration,
    server_ping_timeout: Duration,
    unanswered_ping_limit: u32,
}

impl SocketLiveness {
    pub const fn new(
        ping_interval: Duration,
        client_ping_timeout: Duration,
        server_ping_timeout: Duration,
        unanswered_ping_limit: u32,
    ) -> Self {
        Self {
            ping_interval,
            client_ping_timeout,
            server_ping_timeout,
            unanswered_ping_limit,
        }
    }

    pub fn ping_interval(self) -> Duration {
        self.ping_interval
    }

    pub fn client_ping_timeout(self) -> Duration {
        self.client_ping_timeout
    }

    pub fn server_ping_timeout(self) -> Duration {
        self.server_ping_timeout
    }
}

#[derive(Debug)]
pub struct PongWatch {
    timeout: Duration,
    unanswered_window: Duration,
    answered: bool,
    oldest_unanswered_ping: Option<Instant>,
}

impl PongWatch {
    pub fn new(liveness: SocketLiveness) -> Self {
        Self {
            timeout: liveness.client_ping_timeout,
            unanswered_window: liveness.ping_interval.saturating_mul(liveness.unanswered_ping_limit),
            answered: false,
            oldest_unanswered_ping: None,
        }
    }

    pub fn pong_received(&mut self) {
        self.answered = true;
        self.oldest_unanswered_ping = None;
    }

    pub fn ping_sent(&mut self, now: Instant) {
        self.oldest_unanswered_ping.get_or_insert(now);
    }

    pub fn stall_deadline(&self) -> Option<Instant> {
        let window = if self.answered {
            self.timeout
        } else {
            self.unanswered_window
        };
        self.oldest_unanswered_ping.map(|sent| sent + window)
    }
}

#[cfg(test)]
mod tests;
