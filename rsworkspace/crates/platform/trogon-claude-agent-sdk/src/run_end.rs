//! When stdin closes: the run-end machine of the upstream SDKs.
//!
//! `sdk.mjs` (`runEnded`, `resultReceived`, `endRun`, `armRunEndCeiling`, `waitForRunEnd`) and Python's
//! `Query` (`_run_ended_event`, `wait_for_result_and_end_input`, `_arm_run_end_ceiling`) decide from the frames
//! alone whether the CLI may still ask the SDK something. This holds exactly that and nothing else, so a
//! condition upstream lacks cannot move the moment stdin closes.

use std::{collections::HashSet, time::Duration};

use tokio::time::Instant;

use crate::{ArgvStyle, Message};

#[derive(Clone, Copy, PartialEq, Eq)]
enum SessionState {
    Idle,
    Running,
    RequiresAction,
    /// A state this SDK does not know yet. Both upstream SDKs treat it as not idle.
    Other,
}

pub(crate) struct RunEnd {
    style: ArgvStyle,
    /// `runEndCeilingMs` and Python's `_run_end_ceiling_ms`; `None` is 0 or less, which never arms it.
    ceiling: Option<Duration>,
    /// TypeScript `runEnded`, Python `_run_ended_event.is_set()`.
    ended: bool,
    /// TypeScript `resultReceived`, Python `_result_received`.
    result_received: bool,
    /// The last `session_state_changed` state.
    state: Option<SessionState>,
    /// Python `_turn_in_progress`.
    turn_in_progress: bool,
    /// Python `_inflight_tasks`.
    in_flight: HashSet<String>,
    /// Python `_run_final`: stdin is closed or the reader is gone, so the run stays ended.
    run_final: bool,
    /// Prompts written on the session.
    written: usize,
    /// Prompts the finished input wrote, TypeScript `streamInput`'s `count` and Python's `written`.
    input_written: usize,
    /// The input is complete: `streamInput` finished writing, or Python spawned its waiter.
    finished: bool,
    /// TypeScript `isSingleUserTurn`: a string prompt, whose stdin closes inside `endRun`.
    single_turn: bool,
    /// The run-end wait `streamInput` or the Python waiter entered after the last write.
    holding: bool,
    /// The wait was released: the run ended while the waiter was waiting.
    released: bool,
    /// `endRun` closed stdin of a single user turn.
    close_now: bool,
    deadline: Option<Instant>,
}

impl RunEnd {
    pub(crate) fn new(style: ArgvStyle, ceiling: Option<Duration>) -> Self {
        Self {
            style,
            ceiling,
            ended: false,
            result_received: false,
            state: None,
            turn_in_progress: false,
            in_flight: HashSet::new(),
            run_final: false,
            written: 0,
            input_written: 0,
            finished: false,
            single_turn: false,
            holding: false,
            released: false,
            close_now: false,
            deadline: None,
        }
    }

    fn typescript(&self) -> bool {
        self.style == ArgvStyle::TypeScript
    }

    /// The pending ceiling, when one is armed.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Stdin must close now.
    pub(crate) fn close_due(&self) -> bool {
        self.close_now || (self.finished && !self.single_turn && (!self.holding || self.released))
    }

    /// The input waiter decided to close stdin. Python sets `_run_final` in `wait_for_result_and_end_input` right
    /// after the run-end wait, with the ceiling cleared, and in `stream_input` when nothing was written, where the
    /// ceiling is left alone. TypeScript has no such state and only drops the pending ceiling.
    pub(crate) fn closing_input(&mut self) {
        if self.typescript() {
            self.deadline = None;
            return;
        }
        self.run_final = true;
        if self.input_written > 0 {
            self.deadline = None;
        }
    }

    /// Python `_read_messages`' `finally`: the reader is gone, so `_run_final` is set and the run ends.
    pub(crate) fn reader_gone(&mut self) {
        if !self.typescript() {
            self.run_final = true;
            self.end_run();
        }
    }

    /// A prompt is about to be written: TypeScript `writeStreamMessage`, Python `stream_input`.
    pub(crate) fn prompt_written(&mut self) {
        self.written += 1;
        if self.typescript() {
            self.ended = false;
        } else {
            self.reopen();
        }
        self.result_received = false;
        self.deadline = None;
    }

    /// The caller finished its input. `single_turn` is TypeScript's string prompt; `needs` is
    /// `hasBidirectionalNeeds()`; `written` is the caller's own count of the prompts it wrote, every prompt of the
    /// session when it kept none.
    pub(crate) fn finish(&mut self, single_turn: bool, needs: bool, written: Option<usize>) {
        self.finished = true;
        self.input_written = written.unwrap_or(self.written);
        if self.typescript() && single_turn {
            self.single_turn = true;
            if self.ended {
                self.close_now = true;
            }
            return;
        }
        // `if (count > 0 && hasBidirectionalNeeds()) await waitForRunEnd()`, and Python's `stream_input` and
        // string prompt wait the same way.
        self.holding = self.input_written > 0 && needs;
        if self.ended {
            self.released = true;
        }
    }

    fn reopen(&mut self) {
        if self.ended && !self.run_final {
            self.ended = false;
        }
    }

    fn end_run(&mut self) {
        self.deadline = None;
        if self.typescript() && self.ended {
            return;
        }
        self.ended = true;
        if self.finished {
            self.released = true;
        }
        if self.single_turn {
            self.close_now = true;
        }
    }

    fn arm(&mut self, needs: bool) {
        self.deadline = None;
        let skip = self.ceiling.is_none() || self.ended;
        let skip = skip
            || if self.typescript() {
                false
            } else {
                self.run_final || self.turn_in_progress || !needs
            };
        if !skip {
            self.deadline = self.ceiling.and_then(|ceiling| Instant::now().checked_add(ceiling));
        }
    }

    /// The ceiling passed with no new turn.
    pub(crate) fn ceiling_elapsed(&mut self) {
        self.deadline = None;
        if !self.typescript() && !self.in_flight.is_empty() {
            tracing::debug!(
                tasks = self.in_flight.len(),
                "no idle state after the last result, but tracked tasks are still in flight; keeping stdin open"
            );
            return;
        }
        tracing::debug!(timeout = ?self.ceiling, "no idle state after the last result; ending the run");
        self.end_run();
    }

    /// Python `_rearm_run_end_ceiling_between_turns`.
    fn rearm_between_turns(&mut self, needs: bool) {
        if self.result_received && matches!(self.state, Some(SessionState::Running | SessionState::Other)) {
            self.arm(needs);
        }
    }

    /// Python `_maybe_end_run`.
    fn maybe_end_run(&mut self) {
        if !self.in_flight.is_empty() {
            return;
        }
        self.end_run();
    }

    /// One message the CLI sent. `needs` is `hasBidirectionalNeeds()` now.
    pub(crate) fn observe(&mut self, message: &Message, needs: bool) {
        match message {
            Message::Result(_) => self.result(needs),
            Message::Assistant(message) if message.parent_tool_use_id.is_none() => self.turn_activity(),
            Message::StreamEvent(message) if message.parent_tool_use_id.is_none() => self.turn_activity(),
            Message::System(message) => {
                if !self.typescript() {
                    let had_tasks = !self.in_flight.is_empty();
                    self.track_task(&message.subtype, &message.data);
                    if had_tasks && self.in_flight.is_empty() {
                        self.rearm_between_turns(needs);
                    }
                }
                if message.subtype == "session_state_changed" {
                    let state = match message.data.get("state").and_then(serde_json::Value::as_str) {
                        Some("idle") => SessionState::Idle,
                        Some("running") => SessionState::Running,
                        Some("requires_action") => SessionState::RequiresAction,
                        _ => SessionState::Other,
                    };
                    self.session_state(state, needs);
                }
            }
            _ => {}
        }
    }

    fn result(&mut self, needs: bool) {
        self.result_received = true;
        if self.typescript() {
            if matches!(self.state, None | Some(SessionState::Idle)) || !needs {
                self.end_run();
            } else {
                self.arm(needs);
            }
            return;
        }
        self.turn_in_progress = false;
        if matches!(self.state, None | Some(SessionState::Idle)) || !needs {
            self.maybe_end_run();
        } else if self.state != Some(SessionState::RequiresAction) {
            self.arm(needs);
        }
    }

    fn turn_activity(&mut self) {
        if self.typescript() {
            if !(self.single_turn && self.ended) {
                self.ended = false;
                self.deadline = None;
            }
            return;
        }
        self.turn_in_progress = true;
        self.reopen();
        self.deadline = None;
    }

    fn session_state(&mut self, state: SessionState, needs: bool) {
        self.state = Some(state);
        if state == SessionState::Idle {
            if self.result_received {
                if self.typescript() {
                    self.end_run();
                } else {
                    self.maybe_end_run();
                }
            }
            return;
        }
        if self.typescript() {
            if !(self.single_turn && self.ended) {
                self.ended = false;
                if state == SessionState::RequiresAction {
                    self.deadline = None;
                } else if self.result_received {
                    self.arm(needs);
                }
            }
            return;
        }
        self.reopen();
        if state == SessionState::RequiresAction {
            self.deadline = None;
        } else {
            self.rearm_between_turns(needs);
        }
    }

    /// Python `_track_task_lifecycle`: only delegated agent work is tracked.
    fn track_task(&mut self, subtype: &str, data: &crate::JsonObject) {
        let Some(id) = data
            .get("task_id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        match subtype {
            "task_started" => {
                if matches!(
                    data.get("task_type").and_then(serde_json::Value::as_str),
                    Some("local_agent" | "local_workflow")
                ) {
                    self.in_flight.insert(id.to_owned());
                }
            }
            "task_notification" => {
                self.in_flight.remove(id);
            }
            "task_updated" => {
                let status = data
                    .get("patch")
                    .and_then(|patch| patch.get("status"))
                    .and_then(serde_json::Value::as_str);
                if matches!(status, Some("completed" | "failed" | "stopped" | "killed")) {
                    self.in_flight.remove(id);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "run_end_tests.rs"]
mod tests;
