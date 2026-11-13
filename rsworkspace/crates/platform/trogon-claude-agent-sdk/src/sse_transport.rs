//! The TypeScript browser SDK's SSE transport as an implementation of [`Transport`].

use std::{
    collections::VecDeque,
    future::pending,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use futures_util::StreamExt;
use indexmap::IndexMap;
use reqwest::{
    Client, Response,
    header::{HeaderMap, HeaderName, HeaderValue, RETRY_AFTER},
};
use serde_json::Value;
use tokio::{
    sync::{Notify, mpsc, watch},
    time::{Instant, sleep, sleep_until, timeout, timeout_at},
};
use tokio_util::sync::CancellationToken;

use crate::{
    AbortError, Error, JsonObject, SseCcrEvent, SseError, SseEvent, SseFrame, SseFrameFilter, SseHttpError,
    SseLogLevel, SseLogRecord, SseOptions, SseParser, Transport, http_error_reason,
    sse_retry::{
        CONNECT_TIMEOUT, LIVENESS_TIMEOUT, POST_TIMEOUT, RECONNECT_BUDGET, jitter, reconnect_delay, retry_after_ms,
        retry_delay,
    },
};

/// TypeScript `d4`: the text envelope arm of the browser transport is ported.
pub const G4_TEXT_ENVELOPE_ARM_PORTED: bool = true;

/// TypeScript `s4`: lines waiting to be written before a writer has to wait.
const QUEUE_LIMIT: usize = 1_000;
/// TypeScript `i4`: failed attempts in a row after which a line is dropped.
const UPLOAD_ATTEMPTS: u32 = 3;
/// How much of an error response is read to find its reason.
const REASON_BYTES: usize = 4_096;

type Delivery = JsonObject;

/// TypeScript `envelopesByMessage`: the envelope each durable message arrived in. TypeScript keys a `WeakMap` by the
/// message object and keeps an entry as long as the message lives. A Rust message is moved to the consumer, so an
/// entry is found by the message's `uuid` when it has one and by equal JSON when it has none, and entries are kept
/// for the life of the transport.
#[derive(Default)]
struct Envelopes {
    by_uuid: std::collections::HashMap<String, SseCcrEvent>,
    unkeyed: Vec<(JsonObject, SseCcrEvent)>,
}

impl Envelopes {
    fn record(&mut self, message: &JsonObject, event: SseCcrEvent) {
        match message.get("uuid").and_then(Value::as_str) {
            Some(uuid) => {
                self.by_uuid.insert(uuid.to_owned(), event);
            }
            None => self.unkeyed.push((message.clone(), event)),
        }
    }

    fn find(&self, message: &JsonObject) -> Option<&SseCcrEvent> {
        match message.get("uuid").and_then(Value::as_str) {
            Some(uuid) => self.by_uuid.get(uuid),
            None => self
                .unkeyed
                .iter()
                .rev()
                .find(|(delivered, _)| delivered == message)
                .map(|(_, event)| event),
        }
    }
}
type Ready = Option<Result<(), SseError>>;

struct State {
    filter: SseFrameFilter,
    exit_error: Option<SseError>,
    envelopes: Envelopes,
    ready: bool,
    suspended: bool,
    /// Counts suspensions, so a waiting reconnect can tell it was suspended and resumed.
    suspensions: u64,
    reconnect_attempts: u32,
    stream: Option<CancellationToken>,
    queue: VecDeque<String>,
    events: Option<mpsc::UnboundedSender<Delivery>>,
}

enum Begin {
    Closed,
    Suspended,
    Run(CancellationToken, u64),
}

enum Outcome {
    /// The stream was suspended or the transport closed while the attempt ran.
    Interrupted,
    /// The stream ended or could not be opened; a reconnect may follow.
    Ended(Option<SseError>),
    /// The stream cannot be opened by retrying.
    Failed(SseError),
}

struct Shared {
    options: SseOptions,
    client: Client,
    abort: Option<CancellationToken>,
    /// Divides every wait; 1 except in tests.
    divisor: u32,
    state: Mutex<State>,
    closed: CancellationToken,
    wake: Notify,
    uploads: Notify,
    room: Notify,
    first_ready: watch::Sender<Ready>,
}

async fn cancelled(token: Option<&CancellationToken>) {
    match token {
        Some(token) => token.cancelled().await,
        None => pending().await,
    }
}

fn log(record: &SseLogRecord) {
    match record.level {
        SseLogLevel::Debug => tracing::debug!("{}", record.text),
        SseLogLevel::Warn => tracing::warn!("{}", record.text),
    }
}

fn call_handler(name: &str, handler: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(handler)).is_err() {
        tracing::warn!("[BrowserSSETransport] {name} handler threw");
    }
}

/// What Node's `fetch` reports when a request fails before an answer: an unparsable URL, or `fetch failed`.
fn request_failure(url: &str) -> SseError {
    if url::Url::parse(url).is_err() {
        SseError::Fetch(format!("Failed to parse URL from {url}"))
    } else {
        SseError::Fetch("fetch failed".to_owned())
    }
}

/// TypeScript `nA`: the reason in the first 4096 bytes of an error response, if it names one.
async fn error_reason(mut response: Response) -> Option<String> {
    let mut body = Vec::new();
    while body.len() < REASON_BYTES {
        let Some(chunk) = response.chunk().await.ok()? else {
            break;
        };
        let take = chunk.len().min(REASON_BYTES - body.len());
        body.extend_from_slice(chunk.get(..take).unwrap_or_default());
    }
    http_error_reason(&body)
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn scaled(&self, duration: Duration) -> Duration {
        duration / self.divisor
    }

    fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }

    fn settle_ready(&self, outcome: Result<(), SseError>) {
        self.first_ready.send_if_modified(|value| {
            let pending = value.is_none();
            if pending {
                *value = Some(outcome);
            }
            pending
        });
    }

    /// TypeScript `close`.
    fn close(&self) {
        let mut state = self.state();
        self.close_locked(&mut state);
    }

    fn close_locked(&self, state: &mut State) {
        if self.is_closed() {
            return;
        }
        self.closed.cancel();
        if !state.ready {
            let error = state.exit_error.clone().unwrap_or(SseError::ClosedBeforeConnect);
            self.settle_ready(Err(error));
        }
        state.ready = false;
        if let Some(stream) = state.stream.take() {
            stream.cancel();
        }
        state.queue.clear();
        state.events = None;
    }

    /// TypeScript `fail`: the first error stays the one the stream ends with.
    fn fail(&self, error: SseError) {
        let mut state = self.state();
        if state.exit_error.is_none() {
            state.exit_error = Some(error);
        }
        self.close_locked(&mut state);
    }

    /// TypeScript `abortHandler`: the abort error replaces whatever error was recorded.
    fn on_abort(&self) {
        let mut state = self.state();
        state.exit_error = Some(SseError::Aborted);
        self.close_locked(&mut state);
    }

    /// TypeScript `suspend`.
    fn suspend(&self) -> bool {
        let mut state = self.state();
        if self.is_closed() || state.suspended {
            return false;
        }
        state.suspended = true;
        state.suspensions += 1;
        state.ready = false;
        if let Some(stream) = state.stream.take() {
            stream.cancel();
        }
        true
    }

    /// TypeScript `resume`.
    fn resume(&self) -> bool {
        {
            let mut state = self.state();
            if self.is_closed() || !state.suspended {
                return false;
            }
            state.suspended = false;
        }
        self.wake.notify_one();
        true
    }

    fn expect_control_response(&self, request_id: &str) {
        self.resume();
        let record = self.state().filter.expect_control_response(request_id);
        if let Some(record) = record {
            log(&record);
        }
    }

    /// TypeScript `buildHeaders`: the configured headers with `extra` spread over them, so an extra header takes the
    /// place of a configured one spelled the same. Headers that differ only in case are joined with a comma, as
    /// `fetch` does.
    fn request_headers(&self, extra: &[(&str, String)]) -> Result<HeaderMap, SseError> {
        let spelled = |name: &str| extra.iter().find(|(extra_name, _)| *extra_name == name);
        let configured = self.options.headers.iter().map(|(name, value)| match spelled(name) {
            Some((_, replacement)) => (name.as_str(), replacement.as_str()),
            None => (name.as_str(), value.as_str()),
        });
        let appended = extra
            .iter()
            .filter(|(name, _)| !self.options.headers.contains_key(*name))
            .map(|(name, value)| (*name, value.as_str()));
        let mut merged: IndexMap<HeaderName, String> = IndexMap::new();
        for (name, value) in configured.chain(appended) {
            let header = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| SseError::Fetch(format!("Headers.append: \"{name}\" is an invalid header name.")))?;
            merged
                .entry(header)
                .and_modify(|existing| {
                    existing.push_str(", ");
                    existing.push_str(value);
                })
                .or_insert_with(|| value.to_owned());
        }
        let mut headers = HeaderMap::new();
        for (name, value) in merged {
            let value = HeaderValue::from_str(&value)
                .map_err(|_| SseError::Fetch(format!("Headers.append: \"{value}\" is an invalid header value.")))?;
            headers.insert(name, value);
        }
        Ok(headers)
    }

    fn begin_stream(&self) -> Begin {
        let mut state = self.state();
        if self.is_closed() {
            Begin::Closed
        } else if state.suspended {
            Begin::Suspended
        } else {
            let token = self.closed.child_token();
            state.stream = Some(token.clone());
            Begin::Run(token, state.suspensions)
        }
    }

    /// The loop that opens the stream, reads it and reconnects, until the transport closes.
    async fn supervise(self: Arc<Self>) {
        loop {
            let (token, suspensions) = match self.begin_stream() {
                Begin::Closed => return,
                Begin::Suspended => {
                    tokio::select! {
                        () = self.closed.cancelled() => return,
                        () = self.wake.notified() => {}
                    }
                    continue;
                }
                Begin::Run(token, suspensions) => (token, suspensions),
            };
            let error = match self.stream_once(&token).await {
                Outcome::Interrupted => continue,
                Outcome::Failed(error) => {
                    if !token.is_cancelled() {
                        self.fail(error);
                    }
                    continue;
                }
                Outcome::Ended(error) => error,
            };
            if let Some(delay) = self.stream_ended(&token, error) {
                self.wait_to_reconnect(delay, suspensions).await;
            }
        }
    }

    /// TypeScript `handleStreamEnd`: the wait before the next reconnect, or `None` when none follows.
    fn stream_ended(&self, token: &CancellationToken, error: Option<SseError>) -> Option<Duration> {
        let mut state = self.state();
        state.ready = false;
        if token.is_cancelled() {
            return None;
        }
        if state.reconnect_attempts >= RECONNECT_BUDGET {
            if state.exit_error.is_none() {
                state.exit_error = Some(error.unwrap_or(SseError::BudgetExhausted));
            }
            self.close_locked(&mut state);
            return None;
        }
        state.reconnect_attempts += 1;
        Some(reconnect_delay(state.reconnect_attempts))
    }

    /// Waits out a reconnect delay. A suspension and resume in the meantime ends the wait: the resume reconnects
    /// at once.
    async fn wait_to_reconnect(&self, delay: Duration, suspensions: u64) {
        let deadline = Instant::now() + self.scaled(delay);
        loop {
            tokio::select! {
                () = self.closed.cancelled() => return,
                () = sleep_until(deadline) => return,
                () = self.wake.notified() => {
                    if self.state().suspensions != suspensions {
                        return;
                    }
                }
            }
        }
    }

    /// TypeScript `connect` and `readStream`: one attempt to open the stream and read it to its end.
    async fn stream_once(&self, token: &CancellationToken) -> Outcome {
        let deadline = Instant::now() + self.scaled(CONNECT_TIMEOUT);
        let resume = self.state().filter.resume_sequence_num();
        let mut extra = vec![("Accept", "text/event-stream".to_owned())];
        if resume > 0 {
            extra.push(("Last-Event-ID", resume.to_string()));
        }
        let headers = match self.request_headers(&extra) {
            Ok(headers) => headers,
            Err(error) => return Outcome::Ended(Some(error)),
        };
        let url = &self.options.stream_url;
        let request = self.client.get(url).headers(headers).send();
        let response = tokio::select! {
            biased;
            () = token.cancelled() => return Outcome::Interrupted,
            () = sleep_until(deadline) => return Outcome::Ended(Some(SseError::ConnectTimeout)),
            result = request => match result {
                Ok(response) => response,
                Err(_) => return Outcome::Ended(Some(request_failure(url))),
            },
        };
        let status = response.status();
        if !status.is_success() {
            let reason = tokio::select! {
                biased;
                () = token.cancelled() => return Outcome::Interrupted,
                read = timeout_at(deadline, error_reason(response)) => read.ok().flatten(),
            };
            let error = SseHttpError::new("SSE connect failed", status.as_u16(), reason);
            return if error.is_permanent() {
                Outcome::Failed(error.into())
            } else {
                Outcome::Ended(Some(error.into()))
            };
        }
        if !self.mark_ready(token) {
            return Outcome::Interrupted;
        }
        self.read_stream(response, token).await
    }

    /// Marks the stream ready, unless it was suspended or the transport closed since the attempt began.
    fn mark_ready(&self, token: &CancellationToken) -> bool {
        let mut state = self.state();
        if token.is_cancelled() {
            return false;
        }
        state.ready = true;
        state.reconnect_attempts = 0;
        self.settle_ready(Ok(()));
        true
    }

    async fn read_stream(&self, response: Response, token: &CancellationToken) -> Outcome {
        let mut body = Box::pin(response.bytes_stream());
        let mut parser = SseParser::new();
        let liveness = sleep(self.scaled(LIVENESS_TIMEOUT));
        tokio::pin!(liveness);
        loop {
            tokio::select! {
                biased;
                () = token.cancelled() => return Outcome::Interrupted,
                () = &mut liveness => return Outcome::Ended(Some(SseError::LivenessTimeout)),
                chunk = body.next() => match chunk {
                    None => return Outcome::Ended(None),
                    Some(Err(_)) => return Outcome::Ended(Some(SseError::Fetch("terminated".to_owned()))),
                    Some(Ok(bytes)) => {
                        liveness.as_mut().reset(Instant::now() + self.scaled(LIVENESS_TIMEOUT));
                        for frame in parser.push(&bytes) {
                            if token.is_cancelled() {
                                break;
                            }
                            self.handle_frame(&frame);
                        }
                    }
                },
            }
        }
    }

    fn handle_frame(&self, frame: &SseFrame) {
        let events = self.state().filter.handle_frame(frame);
        for event in events {
            match event {
                SseEvent::Message { message, ccr_event } => self.deliver(message, ccr_event),
                SseEvent::CatchUpTruncated => {
                    if let Some(handler) = &self.options.on_catch_up_truncated {
                        call_handler("onCatchUpTruncated", handler.as_ref());
                    }
                }
                SseEvent::DeliveryUpdate(update) => {
                    if let Some(handler) = &self.options.on_delivery_update {
                        call_handler("onDeliveryUpdate", || handler(update));
                    }
                }
                SseEvent::Log(record) => log(&record),
            }
        }
    }

    fn deliver(&self, message: Value, ccr_event: Option<SseCcrEvent>) {
        let Value::Object(message) = message else {
            return;
        };
        let mut state = self.state();
        if let Some(event) = ccr_event {
            state.envelopes.record(&message, event);
        }
        if let Some(events) = &state.events {
            let _ = events.send(message);
        }
    }

    /// TypeScript `write`.
    async fn write(&self, line: &str) -> Result<(), Error> {
        if cancelled_now(self.abort.as_ref()) {
            return Err(AbortError::OPERATION.into());
        }
        self.resume();
        let mut ready = self.first_ready.subscribe();
        let settled = ready
            .wait_for(Option::is_some)
            .await
            .ok()
            .and_then(|value| value.clone());
        if let Some(Err(error)) = settled {
            return Err(error.into());
        }
        if self.is_closed() {
            let error = self.state().exit_error.clone().unwrap_or(SseError::Closed);
            return Err(error.into());
        }
        let invalid = |error: serde_json::Error| SseError::InvalidJson(error.to_string());
        let prepared = self
            .state()
            .filter
            .prepare_write(line, || uuid::Uuid::new_v4().to_string())
            .map_err(invalid)?;
        let body = prepared.post_body(&self.options.session_id).map_err(invalid)?;
        if let Some(record) = &prepared.log {
            log(record);
        }
        self.enqueue(body).await;
        Ok(())
    }

    /// TypeScript `uploader.enqueue`: waits while the queue is full, and gives the line up when the transport closes.
    async fn enqueue(&self, body: String) {
        loop {
            let room = self.room.notified();
            tokio::pin!(room);
            room.as_mut().enable();
            {
                let mut state = self.state();
                if self.is_closed() {
                    return;
                }
                if state.queue.len() < QUEUE_LIMIT {
                    state.queue.push_back(body);
                    drop(state);
                    self.uploads.notify_one();
                    return;
                }
            }
            tokio::select! {
                () = room => {}
                () = self.closed.cancelled() => {}
            }
        }
    }

    /// The loop that POSTs queued lines one at a time.
    async fn upload(self: Arc<Self>) {
        let mut failures = 0;
        loop {
            if self.is_closed() {
                return;
            }
            let next = self.state().queue.pop_front();
            if next.is_some() {
                self.room.notify_waiters();
            }
            let Some(body) = next else {
                tokio::select! {
                    () = self.closed.cancelled() => return,
                    () = self.uploads.notified() => {}
                }
                continue;
            };
            match self.post_once(body.clone()).await {
                Ok(()) => failures = 0,
                Err(retry_after) => {
                    failures += 1;
                    if self.is_closed() {
                        return;
                    }
                    if failures >= UPLOAD_ATTEMPTS {
                        failures = 0;
                        continue;
                    }
                    self.state().queue.push_front(body);
                    let delay = self.scaled(retry_delay(failures, retry_after, jitter()));
                    tokio::select! {
                        () = self.closed.cancelled() => return,
                        () = sleep(delay) => {}
                    }
                }
            }
        }
    }

    /// TypeScript `postOnce`. The error is the `Retry-After` the server asked for, in milliseconds.
    async fn post_once(&self, body: String) -> Result<(), Option<f64>> {
        let headers = self
            .request_headers(&[("Content-Type", "application/json".to_owned())])
            .map_err(|_| None::<f64>)?;
        tokio::select! {
            biased;
            () = cancelled(self.abort.as_ref()) => Err(None),
            result = timeout(self.scaled(POST_TIMEOUT), self.send_post(headers, body)) => result.unwrap_or(Err(None)),
        }
    }

    async fn send_post(&self, headers: HeaderMap, body: String) -> Result<(), Option<f64>> {
        let url = &self.options.send_url;
        let response = self
            .client
            .post(url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|_| None::<f64>)?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let context = format!("POST {url}");
        if SseHttpError::new(context.clone(), status.as_u16(), None).is_permanent() {
            let reason = error_reason(response).await;
            self.fail(SseHttpError::new(context, status.as_u16(), reason).into());
            return Err(None);
        }
        let header = response
            .headers()
            .get_all(RETRY_AFTER)
            .iter()
            .map(|value| String::from_utf8_lossy(value.as_bytes()))
            .collect::<Vec<_>>()
            .join(", ");
        Err(retry_after_ms(Some(&header)))
    }

    async fn watch_abort(self: Arc<Self>) {
        tokio::select! {
            () = cancelled(self.abort.as_ref()) => self.on_abort(),
            () = self.closed.cancelled() => {}
        }
    }
}

fn cancelled_now(token: Option<&CancellationToken>) -> bool {
    token.is_some_and(CancellationToken::is_cancelled)
}

/// A browser-style SSE connection to a remote Claude Code: lines are read from an event stream and written with
/// POST requests.
///
/// The stream is reopened with `Last-Event-ID` when it drops, up to five times in a row. Written lines wait in
/// a queue of 1000 and are POSTed one at a time, each retried twice before it is dropped. Frames are filtered
/// by [`SseFrameFilter`]. [`SseTransport::handle`] gives the operations the TypeScript query exposes on its
/// transport.
pub struct SseTransport {
    shared: Arc<Shared>,
    events: mpsc::UnboundedReceiver<Delivery>,
    started: bool,
    /// The end of the stream was reported; TypeScript's reader finishes after throwing its error once.
    finished: bool,
}

impl SseTransport {
    pub fn new(options: SseOptions) -> Self {
        let (events_tx, events) = mpsc::unbounded_channel();
        let (first_ready, _) = watch::channel(None);
        let state = State {
            filter: SseFrameFilter::new(options.from_sequence_num),
            exit_error: None,
            envelopes: Envelopes::default(),
            ready: false,
            suspended: false,
            suspensions: 0,
            reconnect_attempts: 0,
            stream: None,
            queue: VecDeque::new(),
            events: Some(events_tx),
        };
        let client = Client::builder().no_proxy().build().unwrap_or_else(|_| Client::new());
        let shared = Shared {
            options,
            client,
            abort: None,
            divisor: 1,
            state: Mutex::new(state),
            closed: CancellationToken::new(),
            wake: Notify::new(),
            uploads: Notify::new(),
            room: Notify::new(),
            first_ready,
        };
        Self {
            shared: Arc::new(shared),
            events,
            started: false,
            finished: false,
        }
    }

    /// TypeScript `abortController`: cancelling the token closes the transport and ends the stream with
    /// [`SseError::Aborted`]; a later write fails with `Operation aborted`. Call before [`SseTransport::handle`].
    #[must_use]
    pub fn with_abort(mut self, token: CancellationToken) -> Self {
        if let Some(shared) = Arc::get_mut(&mut self.shared) {
            shared.abort = Some(token);
        }
        self
    }

    #[cfg(test)]
    fn with_time_divisor(mut self, divisor: u32) -> Self {
        if let Some(shared) = Arc::get_mut(&mut self.shared) {
            shared.divisor = divisor;
        }
        self
    }

    /// The operations the browser query forwards to its transport.
    pub fn handle(&self) -> SseHandle {
        SseHandle {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl Drop for SseTransport {
    /// TypeScript `[Symbol.dispose]`.
    fn drop(&mut self) {
        self.shared.close();
    }
}

impl Transport for SseTransport {
    /// Starts the stream and the writer in the background; readiness is reached later, and a write waits for it.
    async fn connect(&mut self) -> Result<(), Error> {
        if std::mem::replace(&mut self.started, true) {
            return Ok(());
        }
        if cancelled_now(self.shared.abort.as_ref()) {
            self.shared.on_abort();
            return Ok(());
        }
        tokio::spawn(Arc::clone(&self.shared).watch_abort());
        tokio::spawn(Arc::clone(&self.shared).supervise());
        tokio::spawn(Arc::clone(&self.shared).upload());
        Ok(())
    }

    /// Queues the line for POSTing and returns once it is queued, not once the server has it.
    async fn write(&mut self, data: &str) -> Result<(), Error> {
        self.shared.write(data).await
    }

    async fn read_message(&mut self) -> Result<Option<JsonObject>, Error> {
        match self.events.recv().await {
            Some(message) => Ok(Some(message)),
            None => {
                if std::mem::replace(&mut self.finished, true) {
                    return Ok(None);
                }
                match self.shared.state().exit_error.clone() {
                    Some(error) => Err(error.into()),
                    None => Ok(None),
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), Error> {
        self.shared.close();
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.shared.state().ready && !self.shared.is_closed()
    }

    async fn end_input(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn expect_control_response(&mut self, request_id: &str) {
        self.shared.expect_control_response(request_id);
    }
}

/// The operations the TypeScript browser query forwards to its SSE transport.
#[derive(Clone)]
pub struct SseHandle {
    shared: Arc<Shared>,
}

impl SseHandle {
    /// TypeScript `suspendSseStream`: closes the stream and stops reconnecting until a write or
    /// [`SseHandle::resume`]. False when the transport is closed or already suspended.
    pub fn suspend(&self) -> bool {
        self.shared.suspend()
    }

    /// TypeScript `resumeSseStream`: reopens a suspended stream. False when the transport is closed or not
    /// suspended.
    pub fn resume(&self) -> bool {
        self.shared.resume()
    }

    /// TypeScript `expectControlResponse`: resumes a suspended stream, then refuses a non-worker control response
    /// for `request_id`.
    pub fn expect_control_response(&self, request_id: &str) {
        self.shared.expect_control_response(request_id);
    }

    /// TypeScript `getSseDropCounts`: frames dropped per category, empty when none were.
    pub fn drop_counts(&self) -> IndexMap<&'static str, u64> {
        self.shared
            .state()
            .filter
            .drop_counts()
            .into_iter()
            .map(|(category, count)| (category.as_str(), count))
            .collect()
    }

    /// TypeScript `getSseLastSequenceNum`: the highest sequence number of a message the consumer was given through
    /// [`SseHandle::mark_delivered`], or the `from_sequence_num` seed, `None` while that is 0. The transport reading
    /// a frame does not advance it.
    pub fn last_sequence_num(&self) -> Option<u64> {
        self.shared.state().filter.last_sequence_num()
    }

    /// TypeScript `getCcrEvent`: the envelope of the durable frame `message` arrived in. A message without a `uuid`
    /// is found by equal JSON, the latest such delivery first.
    pub fn ccr_event(&self, message: &JsonObject) -> Option<SseCcrEvent> {
        self.shared.state().envelopes.find(message).cloned()
    }

    /// TypeScript `markDelivered`: the consumer received `message`, so its sequence number counts for
    /// [`SseHandle::last_sequence_num`] when it is higher than the current one.
    pub fn mark_delivered(&self, message: &JsonObject) {
        let mut state = self.shared.state();
        let event = state.envelopes.find(message).cloned();
        state.filter.mark_delivered(event.as_ref());
    }
}

#[cfg(test)]
#[path = "sse_transport_tests.rs"]
mod tests;
