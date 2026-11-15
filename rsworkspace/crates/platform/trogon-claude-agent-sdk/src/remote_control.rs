//! TypeScript `enableRemoteControl` and the state it shares with the `remote_control_work_secret` answer.

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
};

use serde_json::Value;

use crate::{CallbackError, CancellationToken, SensitiveString, UiHost};

/// What `refreshWorkSecret` receives: the session id it was attached for, and the request's signal.
#[derive(Clone, Debug)]
pub struct WorkSecretRefreshRequest {
    pub session_id: String,
    pub signal: CancellationToken,
}

pub type WorkSecretFuture<'a> = Pin<Box<dyn Future<Output = Result<Option<String>, CallbackError>> + Send + 'a>>;

/// TypeScript `refreshWorkSecret`: the new secret. `None` and an empty string both answer `null`.
pub trait WorkSecretRefresher: Send + Sync {
    fn call(&self, request: WorkSecretRefreshRequest) -> WorkSecretFuture<'_>;
}

impl<F, Fut> WorkSecretRefresher for F
where
    F: Fn(WorkSecretRefreshRequest) -> Fut + Send + Sync,
    Fut: Future<Output = Result<Option<String>, CallbackError>> + Send + 'static,
{
    fn call(&self, request: WorkSecretRefreshRequest) -> WorkSecretFuture<'_> {
        Box::pin(self(request))
    }
}

impl std::fmt::Debug for dyn WorkSecretRefresher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("WorkSecretRefresher").finish_non_exhaustive()
    }
}

/// TypeScript `enableRemoteControl`'s options.
#[derive(Clone, Debug, Default)]
pub struct RemoteControlOptions {
    /// Sent as `work_secret`; the session id to answer for is derived from it when none is named.
    pub work_secret: Option<SensitiveString>,
    /// Answers the CLI's `remote_control_work_secret` requests for the attached session.
    pub refresh_work_secret: Option<Arc<dyn WorkSecretRefresher>>,
    pub reattach_session_id: Option<String>,
    pub keep_session_on_exit: Option<bool>,
}

/// The session `refreshWorkSecret` answers for.
#[derive(Clone)]
pub(crate) struct WorkSecretAnswerer {
    pub session_id: String,
    pub refresh: Arc<dyn WorkSecretRefresher>,
}

/// What a `ClientHandle` and its driver share: the answerer `enableRemoteControl` installs, the generation
/// that orders its calls, and the host that draws the CLI's prompt.
#[derive(Default)]
pub(crate) struct HostStateInner {
    generation: u64,
    answerer: Option<WorkSecretAnswerer>,
    /// The enable call whose answer the driver settles as it reads the frame, before the next line.
    armed: Option<(u64, [Option<WorkSecretAnswerer>; 2])>,
    ui_host: Option<UiHost>,
}

#[derive(Clone, Default)]
pub(crate) struct HostState(Arc<Mutex<HostStateInner>>);

impl HostState {
    fn lock(&self) -> std::sync::MutexGuard<'_, HostStateInner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn answerer(&self) -> Option<WorkSecretAnswerer> {
        self.lock().answerer.clone()
    }

    pub(crate) fn ui_host(&self) -> Option<UiHost> {
        self.lock().ui_host.clone()
    }

    pub(crate) fn set_ui_host(&self, host: Option<UiHost>) {
        self.lock().ui_host = host;
    }

    /// The start of an `enableRemoteControl` call: takes the next generation, installs `next` and returns the
    /// answerer it replaced.
    pub(crate) fn begin_remote_control(&self, next: Option<WorkSecretAnswerer>) -> (u64, Option<WorkSecretAnswerer>) {
        let mut state = self.lock();
        state.generation += 1;
        (state.generation, std::mem::replace(&mut state.answerer, next))
    }

    /// Records the candidates the driver settles when the CLI's answer to call `generation` arrives, so a
    /// `remote_control_work_secret` request written right behind that answer already sees the settled answerer,
    /// as it does in `sdk.mjs` where the answer's continuation runs before the next line is read.
    pub(crate) fn arm_remote_control(&self, generation: u64, candidates: [Option<WorkSecretAnswerer>; 2]) {
        self.lock().armed = Some((generation, candidates));
    }

    /// The driver's half of an enable: settles the armed call from its answer. A missing or `null` answer leaves
    /// the answerer alone.
    pub(crate) fn settle_remote_control(&self, response: Option<&Value>) {
        let mut state = self.lock();
        let Some((generation, candidates)) = state.armed.take() else {
            return;
        };
        if state.generation != generation {
            return;
        }
        let Some(response) = response.filter(|response| !response.is_null()) else {
            return;
        };
        let bridge_session_id = response.get("bridge_session_id").and_then(Value::as_str);
        state.answerer = bridge_session_id.and_then(|bridge| {
            candidates
                .into_iter()
                .flatten()
                .find(|candidate| same_session(&candidate.session_id, bridge))
        });
    }

    /// A failed call clears the answerer unless a newer call has started.
    pub(crate) fn remote_control_failed(&self, generation: u64) {
        let mut state = self.lock();
        if state.generation == generation {
            state.answerer = None;
        }
    }

    /// A successful enable keeps whichever of `candidates` names the bridge session the CLI attached, unless a
    /// newer call has started. False for a missing or `null` response, the `TypeError` `sdk.mjs` throws reading
    /// `bridge_session_id`, which leaves the answerer installed for the call.
    pub(crate) fn remote_control_enabled(
        &self,
        generation: u64,
        response: Option<&Value>,
        candidates: [Option<WorkSecretAnswerer>; 2],
    ) -> bool {
        let mut state = self.lock();
        if state.generation != generation {
            return true;
        }
        let Some(response) = response.filter(|response| !response.is_null()) else {
            return false;
        };
        let bridge_session_id = response.get("bridge_session_id").and_then(Value::as_str);
        state.answerer = bridge_session_id.and_then(|bridge| {
            candidates
                .into_iter()
                .flatten()
                .find(|candidate| same_session(&candidate.session_id, bridge))
        });
        true
    }
}

/// TypeScript `j8`: the ids are equal, or share a suffix of at least four characters after the last `_`.
pub(crate) fn same_session(left: &str, right: &str) -> bool {
    fn suffix(id: &str) -> &str {
        id.rfind('_').map_or(id, |index| &id[index + 1..])
    }
    if left == right {
        return true;
    }
    let (left, right) = (suffix(left), suffix(right));
    left.encode_utf16().count() >= 4 && left == right
}

/// TypeScript `j8` against a request's `session_id` of any JSON type: a non-string reaches `slice` and throws
/// the engine's `TypeError`.
pub(crate) fn same_requested_session(attached: &str, requested: Option<&Value>) -> Result<bool, String> {
    match requested {
        Some(Value::String(requested)) => Ok(same_session(attached, requested)),
        // An array has `lastIndexOf` and `slice`, and never equals the string suffix.
        Some(Value::Array(_)) => Ok(false),
        None => Err("Cannot read properties of undefined (reading 'slice')".to_owned()),
        Some(Value::Null) => Err("Cannot read properties of null (reading 'slice')".to_owned()),
        Some(_) => Err("t.lastIndexOf is not a function".to_owned()),
    }
}

/// Node's `Buffer.from(text, "base64url")`: both alphabets, invalid characters skipped, decoding stops at `=`
/// and a trailing partial byte is dropped.
fn decode_base64_lenient(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let (mut accumulator, mut bits) = (0_u32, 0_u32);
    for character in text.chars() {
        let value = match character {
            'A'..='Z' => character as u32 - 'A' as u32,
            'a'..='z' => character as u32 - 'a' as u32 + 26,
            '0'..='9' => character as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '=' => break,
            _ => continue,
        };
        accumulator = (accumulator << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push(u8::try_from((accumulator >> bits) & 0xff).unwrap_or_default());
            accumulator &= (1 << bits) - 1;
        }
    }
    bytes
}

fn decode_json_segment(segment: &str) -> Option<Value> {
    serde_json::from_str(&String::from_utf8_lossy(&decode_base64_lenient(segment))).ok()
}

/// TypeScript `dtt`: the session id a work secret carries in its ingress token, or `None` for anything else.
pub(crate) fn work_secret_session_id(work_secret: &str) -> Option<String> {
    let secret = decode_json_segment(work_secret)?;
    let token = secret.as_object()?.get("session_ingress_token")?.as_str()?;
    let token = token.strip_prefix("sk-ant-si-").unwrap_or(token);
    let parts: Vec<&str> = token.split('.').collect();
    let [_, claims, _] = parts.as_slice() else {
        return None;
    };
    if claims.is_empty() {
        return None;
    }
    let claims = decode_json_segment(claims)?;
    let session_id = claims.as_object()?.get("session_id")?.as_str()?;
    (!session_id.is_empty()).then(|| session_id.to_owned())
}

impl RemoteControlOptions {
    /// The answerer `enableRemoteControl` installs for this call, when `enabled` and the options name a session
    /// to answer for.
    pub(crate) fn answerer(&self, enabled: bool) -> Option<WorkSecretAnswerer> {
        let work_secret = self.work_secret.as_ref()?;
        let refresh = self.refresh_work_secret.clone()?;
        let session_id = self
            .reattach_session_id
            .clone()
            .filter(|session_id| !session_id.is_empty())
            .or_else(|| work_secret_session_id(work_secret.expose()))?;
        enabled.then_some(WorkSecretAnswerer { session_id, refresh })
    }
}

#[cfg(test)]
mod tests;
