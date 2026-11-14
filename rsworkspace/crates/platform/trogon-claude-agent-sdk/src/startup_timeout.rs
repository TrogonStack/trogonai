//! The startup initialize timer of `sdk.mjs` 0.3.323 (`class uD`).
//!
//! `startup()` and `prewarm()` wait for the initialize response for a limit. When the caller passes no limit the
//! default is 60 seconds, and time during which Claude Code reports a SessionStart or Setup hook running is not
//! counted, for at most 10 minutes. A limit the caller passes counts throughout.

use std::{collections::HashSet, sync::Arc, time::Duration};

use tokio::time::Instant;

use crate::{
    constants::{STARTUP_DEFAULT_LIMIT, STARTUP_FOLLOWED_LIMIT, STARTUP_HOOK_EVENTS, STARTUP_UNCOUNTED_BUDGET},
    error::InitializeTimeoutError,
};

/// The callback of the runtime-only `onStartupHooksRunning` option: it receives how many reported startup hooks
/// are still running.
pub type StartupHooksRunning = Arc<dyn Fn(usize) + Send + Sync>;

/// What `startup()` and `prewarm()` pass down to a connection.
#[derive(Clone, Default)]
pub(crate) struct StartupSettings {
    /// `initializeTimeoutMs`; `None` is the default limit with uncounted hook time.
    pub initialize_timeout: Option<Duration>,
    pub on_hooks_running: Option<StartupHooksRunning>,
}

/// The receiving end of the frames the driver reports during startup.
pub(crate) struct StartupWatch {
    pub settings: StartupSettings,
    pub events: tokio::sync::mpsc::UnboundedReceiver<StartupEvent>,
}

/// What the driver saw on stdout that the timer or the error report cares about.
#[derive(Clone, Debug)]
pub(crate) enum StartupEvent {
    HookStarted { event: String, id: String },
    HookResponse { event: String, id: String },
    Mark { mark: String, at: Instant },
}

impl StartupEvent {
    /// The event for one raw stdout object, `None` for any other frame.
    pub(crate) fn from_frame(frame: &crate::JsonObject, now: Instant) -> Option<Self> {
        let kind = frame.get("type").and_then(serde_json::Value::as_str)?;
        if kind == "keep_alive" {
            let mark = frame.get("startup_mark")?.as_str()?;
            // JavaScript `length` counts UTF-16 code units.
            return (mark.encode_utf16().count() <= 64).then(|| Self::Mark {
                mark: mark.to_owned(),
                at: now,
            });
        }
        if kind != "system" {
            return None;
        }
        let subtype = frame.get("subtype").and_then(serde_json::Value::as_str)?;
        let event = frame
            .get("hook_event")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        // A missing id is still one id in JavaScript, `undefined`.
        let id = frame
            .get("hook_id")
            .map_or_else(|| "undefined".to_owned(), serde_json::Value::to_string);
        match subtype {
            "hook_started" => Some(Self::HookStarted { event, id }),
            "hook_response" => Some(Self::HookResponse { event, id }),
            _ => None,
        }
    }
}

pub(crate) struct StartupTimer {
    limit: Duration,
    counted_left: Duration,
    uncounted_left: Duration,
    uncounted: Duration,
    counting: bool,
    armed_at: Instant,
    followed: HashSet<String>,
    unfollowed_open: usize,
    reported_events: Vec<&'static str>,
    reported: usize,
    on_running: Option<StartupHooksRunning>,
}

impl StartupTimer {
    /// `limit` is the caller's value; `None` is the default with uncounted hook time.
    pub(crate) fn new(limit: Option<Duration>, now: Instant, on_running: Option<StartupHooksRunning>) -> Self {
        let limit_value = limit.unwrap_or(STARTUP_DEFAULT_LIMIT);
        Self {
            limit: limit_value,
            counted_left: limit_value,
            uncounted_left: if limit.is_none() {
                STARTUP_UNCOUNTED_BUDGET
            } else {
                Duration::ZERO
            },
            uncounted: Duration::ZERO,
            counting: true,
            armed_at: now,
            followed: HashSet::new(),
            unfollowed_open: 0,
            reported_events: Vec::new(),
            reported: 0,
            on_running,
        }
    }

    fn open(&self) -> usize {
        self.followed.len() + self.unfollowed_open
    }

    fn armed_for(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.armed_at)
    }

    fn arm(&mut self, now: Instant) {
        self.armed_at = now;
    }

    /// When the timer next fires: the end of the counted time, or the end of the uncounted time.
    pub(crate) fn deadline(&self) -> Instant {
        let wait = if self.counting {
            self.counted_left
        } else {
            self.uncounted_left
        };
        self.armed_at
            .checked_add(wait)
            .unwrap_or_else(|| self.armed_at + Duration::from_secs(86_400 * 365))
    }

    fn count_again(&mut self, now: Instant) {
        let elapsed = self.armed_for(now).min(self.uncounted_left);
        self.uncounted_left -= elapsed;
        self.uncounted += elapsed;
        self.counting = self.open() == 0 || self.uncounted_left.is_zero();
        self.arm(now);
    }

    /// `note(frame)`: follows the SessionStart and Setup hooks Claude Code reports.
    pub(crate) fn note(&mut self, event: &StartupEvent, now: Instant) {
        match event {
            StartupEvent::HookStarted { event, id } => {
                let Some(name) = STARTUP_HOOK_EVENTS.iter().find(|name| **name == event) else {
                    return;
                };
                if self.followed.contains(id) {
                    return;
                }
                if self.followed.len() < STARTUP_FOLLOWED_LIMIT {
                    self.followed.insert(id.clone());
                } else {
                    self.unfollowed_open += 1;
                }
                if !self.reported_events.contains(name) {
                    self.reported_events.push(name);
                }
                self.reported += 1;
                if self.counting && !self.uncounted_left.is_zero() {
                    self.counted_left = self.counted_left.saturating_sub(self.armed_for(now));
                    self.counting = false;
                    self.arm(now);
                }
                self.report();
            }
            StartupEvent::HookResponse { event, id } => {
                if !self.followed.remove(id) {
                    if self.unfollowed_open == 0 || !STARTUP_HOOK_EVENTS.contains(&event.as_str()) {
                        return;
                    }
                    self.unfollowed_open -= 1;
                }
                if !self.counting {
                    self.count_again(now);
                }
                self.report();
            }
            StartupEvent::Mark { .. } => {}
        }
    }

    fn report(&self) {
        if let Some(callback) = &self.on_running {
            callback(self.open());
        }
    }

    /// The deadline passed: uncounted time ends and counting resumes, or the counted time ran out.
    pub(crate) fn fire(&mut self, now: Instant) -> Option<InitializeTimeoutError> {
        if self.counting {
            return Some(self.expire());
        }
        self.count_again(now);
        None
    }

    fn expire(&self) -> InitializeTimeoutError {
        let events = STARTUP_HOOK_EVENTS.join(" or ");
        // `Math.round(uncountedMs / 1000)`: halves round up.
        let rounded = (self.uncounted.as_millis() + 500) / 1000;
        let open = self.open();
        let summary = format!(
            "{} {} {}",
            self.reported,
            self.reported_events.join(" and "),
            plural(self.reported == 1, "hook")
        );
        let mut parts = vec![format!(
            "Subprocess initialization did not complete within {}ms.",
            self.limit.as_millis()
        )];
        if rounded > 0 {
            parts.push(format!(
                "Not counted: {rounded} {} in which Claude Code reported a {events} hook running.",
                plural(rounded == 1, "second")
            ));
        }
        parts.push(if self.reported == 0 {
            format!(
                "Claude Code did not answer the initialize request and reported no {events} hook running. Slow plugins, MCP servers, hooks that Claude Code does not report, disks or network can each cause this."
            )
        } else if open == 0 {
            format!(
                "Claude Code reported {summary}, which {}finished, and then did not answer the initialize request. Slow plugins, MCP servers, hooks that Claude Code does not report, disks or network can each cause this.",
                if self.reported == 1 { "" } else { "all " }
            )
        } else {
            let unfinished = if self.reported == 1 {
                "which had not finished".to_owned()
            } else if open == self.reported {
                "none of which had finished".to_owned()
            } else {
                format!("{open} of which had not finished")
            };
            format!(
                "Claude Code reported {summary}, {unfinished}, and did not answer the initialize request. A hook that runs in the background counts as not finished until it ends. Check {} first, then authentication and network connectivity.",
                if self.reported == 1 { "this hook" } else { "these hooks" }
            )
        });
        InitializeTimeoutError::new(parts.join(" "), open)
    }
}

fn plural(single: bool, word: &str) -> String {
    if single { word.to_owned() } else { format!("{word}s") }
}

#[cfg(test)]
#[path = "startup_timeout_tests.rs"]
mod tests;
