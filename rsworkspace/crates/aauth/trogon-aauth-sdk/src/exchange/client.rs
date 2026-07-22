//! Thin `reqwest` driver for the sans-io core in [`crate::exchange::core`].
//!
//! [`PsTokenClient`] builds the initial signed POST to a PS's `token_endpoint`
//! (delegating header construction to an [`HttpRequestSigner`]), sends it,
//! translates the HTTP response into an [`ExchangeEvent`], feeds it to
//! [`step`], and loops (sleeping per `Retry-After`/backoff) until the core
//! reaches a state the caller must act on. Resuming (once the caller/user
//! has acted on an interaction or clarification prompt) is exposed as
//! explicit methods (`poll`, `respond_to_clarification`,
//! `submit_updated_request`, `cancel`) rather than folding everything into
//! one mega-enum -- callers already have the `location` from the surfaced
//! state, and explicit methods keep each HTTP verb (GET/POST/DELETE) and
//! its typed request/response shape visible at the call site instead of
//! behind a generic "resume" dispatch.
//!
//! ## HTTP signing is intentionally partial
//!
//! Requests to a PS's `token_endpoint` (and the pending-URL follow-ups) are
//! HTTP, not NATS, so [`crate::signer::AgentSigner::sign_nats_request`]
//! does not apply -- that method produces the Trogon-defined NATS envelope,
//! not RFC 9421 HTTP Message Signatures. This crate has no existing HTTP-side
//! signer to reuse, and implementing full RFC 9421 (`Signature-Input`edge
//! canonicalization, detached signature bytes over covered components) is a
//! separate, larger effort tracked as follow-up against the draft's
//! `(#http-message-signatures-profile)` section.
//!
//! [`HttpRequestSigner`] is deliberately swappable so that future signer can
//! be dropped in without changing [`PsTokenClient`]'s structure. The only
//! concrete implementation shipped here, [`SignatureKeyOnlyHttpSigner`], sets
//! the real, spec-defined `Signature-Key: sig=jwt;jwt="<token>"` bearer-style
//! header per "Agent Token Request" / "Auth Token Usage", but does NOT
//! compute the accompanying `Signature-Input`/`Signature` detached signature
//! envelope. Its name says so on purpose -- do not mistake it for a complete
//! RFC 9421 implementation.

use std::time::Duration;

use trogon_identity_types::aauth::error::{ErrorResponse, PollingError, TokenEndpointError};
use trogon_identity_types::aauth::headers;
use trogon_identity_types::aauth::person_server::{
    ClarificationAction, ClarificationResponseRequest, PendingResponse, TokenGrantResponse, TokenRequest,
    UpdatedRequest,
};

use super::core::{ExchangeAction, ExchangeEvent, ExchangeState, PendingHeaders, TerminalReason, step};
use base64::Engine;

/// Errors from driving an exchange over HTTP.
#[derive(Debug, thiserror::Error)]
pub enum ExchangeError {
    #[error("http transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("token endpoint error: {0:?}")]
    TokenEndpoint(TokenEndpointError),
    #[error("exchange abandoned after {polls} polls without a terminal response")]
    PollBudgetExhausted { polls: u32 },
    #[error("token endpoint returned an error body this SDK does not model: {0:?}")]
    UnrecognizedTokenEndpointError(Box<ErrorResponse>),
    #[error("response body could not be decoded as JSON: {0}")]
    MalformedBody(#[source] reqwest::Error),
    #[error("request body could not be encoded as JSON: {0}")]
    MalformedRequestBody(#[source] serde_json::Error),
    #[error("202 response is missing the required Location header")]
    MissingLocation,
    #[error("202 response is missing the required Retry-After header")]
    MissingRetryAfter,
    #[error("updated request's resource token claims (iss/agent/agent_jkt) do not match the original resource token")]
    UpdatedRequestClaimsMismatch,
    #[error("resource token could not be decoded to compare claims: {0}")]
    MalformedResourceToken(&'static str),
    #[error("server returned an unexpected status code {0} for this step of the exchange")]
    UnexpectedStatus(u16),
}

/// Terminal outcome of an exchange, surfaced to the caller once the sans-io
/// core reaches a state that needs attention. Carries enough context
/// (`location`) that the caller can resume via [`PsTokenClient`]'s explicit
/// methods once the user/caller has acted.
#[derive(Debug)]
pub enum ExchangeOutcome {
    /// Direct grant, or the eventual result of a poll/clarification loop.
    Granted(TokenGrantResponse),
    /// `requirement=interaction` surfaced distinctly with url+code, per
    /// "User Interaction". Direct the user to `{url}?code={code}`, then call
    /// [`PsTokenClient::poll`] with `poll_location`.
    Interaction {
        url: String,
        code: Option<String>,
        poll_location: String,
    },
    /// A clarification question surfaced distinctly, per "Clarification
    /// Chat". Respond with exactly one of
    /// [`PsTokenClient::respond_to_clarification`],
    /// [`PsTokenClient::submit_updated_request`], or
    /// [`PsTokenClient::cancel`].
    Clarification {
        question: String,
        timeout: Option<i64>,
        options: Option<Vec<String>>,
        poll_location: String,
    },
    /// Still pending with no requirement surfaced (including
    /// `status=interacting`, where the agent should stop actively prompting
    /// the user but keep polling). Call [`PsTokenClient::poll`] again.
    Pending { poll_location: String },
    /// `403` denied/abandoned, `408` expired, or `410` gone -- see
    /// [`TerminalReason`] for which. `408` MAY start a fresh request; the
    /// others (including `410`) MUST NOT be retried.
    Denied(TerminalReason),
    /// A typed error occurred (transport failure, malformed response, or a
    /// token-endpoint error code).
    Error(ExchangeError),
}

/// Trait for attaching an HTTP-signing credential to an outbound request.
/// Swappable so a future full RFC 9421 signer can replace
/// [`SignatureKeyOnlyHttpSigner`] without changing [`PsTokenClient`].
pub trait HttpRequestSigner {
    /// Apply signing headers to `builder` for a request whose canonical body
    /// bytes are `body` (empty for GET/DELETE).
    fn sign(&self, builder: reqwest::RequestBuilder, body: &[u8]) -> reqwest::RequestBuilder;
}

/// Partial [`HttpRequestSigner`] that sets only the bearer-style
/// `Signature-Key: sig=jwt;jwt="<token>"` header per "Agent Token Request" /
/// "Auth Token Usage". Does NOT compute the RFC 9421 `Signature-Input` /
/// `Signature` detached-signature envelope -- see the module doc comment.
/// Named explicitly so it is never mistaken for a complete implementation.
pub struct SignatureKeyOnlyHttpSigner {
    token: String,
}

impl SignatureKeyOnlyHttpSigner {
    /// `token` is the agent's own `aa-agent+jwt` for the initial token
    /// request, per "Agent Token Request". Trogon's NATS binding does not
    /// swap this for the auth token on later requests (see
    /// [`crate::signer::AgentSigner::with_auth_token`] doc comment for why);
    /// this HTTP path mirrors the literal draft wire format instead, where
    /// `Signature-Key` does carry whichever token is being presented for
    /// that specific request.
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self { token: token.into() }
    }
}

impl HttpRequestSigner for SignatureKeyOnlyHttpSigner {
    fn sign(&self, builder: reqwest::RequestBuilder, _body: &[u8]) -> reqwest::RequestBuilder {
        builder.header(headers::SIGNATURE_KEY, format!("sig=jwt;jwt=\"{}\"", self.token))
    }
}

/// Thin async driver over [`crate::exchange::core`], wrapping a
/// `reqwest::Client` and an [`HttpRequestSigner`].
pub struct PsTokenClient<S: HttpRequestSigner> {
    http: reqwest::Client,
    signer: S,
    token_endpoint: String,
    wait_secs: u64,
    /// Floor applied to every poll sleep so a PS answering
    /// `Retry-After: 0` cannot turn the loop into an unthrottled storm.
    min_poll_interval_secs: u64,
    /// Hard cap on poll round-trips before the exchange fails cleanly
    /// instead of running forever against a PS that never resolves.
    max_poll_iterations: u32,
}

impl<S: HttpRequestSigner> PsTokenClient<S> {
    /// Encodes a request body as JSON bytes, mapping serialization failure
    /// to [`ExchangeError::MalformedRequestBody`]. Shared by
    /// [`Self::post_initial_request`] and [`Self::post_clarification_action`]
    /// so the one early-return call site -- unreachable for this SDK's
    /// plain-field request types -- is exercisable via
    /// [`Self::post_clarification_action`] with a deliberately failing
    /// `Serialize` impl, rather than duplicated at each call site and left
    /// dead everywhere else.
    fn encode_request_body(value: &impl serde::Serialize) -> Result<Vec<u8>, ExchangeError> {
        serde_json::to_vec(value).map_err(ExchangeError::MalformedRequestBody)
    }

    #[must_use]
    pub fn new(http: reqwest::Client, signer: S, token_endpoint: impl Into<String>) -> Self {
        Self {
            http,
            signer,
            token_endpoint: token_endpoint.into(),
            wait_secs: 45,
            min_poll_interval_secs: 1,
            max_poll_iterations: 300,
        }
    }

    /// Override the minimum poll sleep. Zero disables the floor -- meant
    /// for tests against a local mock, not production use.
    #[must_use]
    pub fn with_min_poll_interval_secs(mut self, secs: u64) -> Self {
        self.min_poll_interval_secs = secs;
        self
    }

    /// Override the maximum number of poll round-trips before the exchange
    /// fails with [`ExchangeError::PollBudgetExhausted`].
    #[must_use]
    pub fn with_max_poll_iterations(mut self, iterations: u32) -> Self {
        self.max_poll_iterations = iterations;
        self
    }

    /// Override the `Prefer: wait=N` value sent on requests. Defaults to 45,
    /// matching the draft's "Agent Token Request" example.
    #[must_use]
    pub fn with_wait_secs(mut self, wait_secs: u64) -> Self {
        self.wait_secs = wait_secs;
        self
    }

    /// Sends the initial signed POST to the PS's `token_endpoint` and drives
    /// the exchange until a state the caller must act on is reached.
    pub async fn request_token(&self, request: &TokenRequest) -> ExchangeOutcome {
        self.post_initial_request(request).await
    }

    /// Encodes and sends the initial signed POST. Generic over the body
    /// type (rather than inlined into [`Self::request_token`]) so the
    /// encode-failure arm -- unreachable for [`TokenRequest`]'s plain
    /// fields -- shares its source line with [`Self::post_clarification_action`]'s
    /// identical arm and is exercised by the same failing-`Serialize` test.
    async fn post_initial_request(&self, request: &impl serde::Serialize) -> ExchangeOutcome {
        let body = match Self::encode_request_body(request) {
            Ok(b) => b,
            Err(e) => return ExchangeOutcome::Error(e),
        };
        let builder = self
            .http
            .post(&self.token_endpoint)
            .header("Prefer", format!("wait={}", self.wait_secs))
            .json(request);
        let builder = self.signer.sign(builder, &body);

        let response = match builder.send().await {
            Ok(r) => r,
            Err(e) => return ExchangeOutcome::Error(ExchangeError::Transport(e)),
        };
        self.handle_response(response, ExchangeState::AwaitingInitial).await
    }

    /// Resumes polling `location` (the `poll_location` from a prior
    /// [`ExchangeOutcome`]) with a GET, per "Polling with GET".
    pub async fn poll(&self, location: &str) -> ExchangeOutcome {
        let builder = self
            .http
            .get(location)
            .header("Prefer", format!("wait={}", self.wait_secs));
        let builder = self.signer.sign(builder, &[]);
        let response = match builder.send().await {
            Ok(r) => r,
            Err(e) => return ExchangeOutcome::Error(ExchangeError::Transport(e)),
        };
        self.handle_response(
            response,
            ExchangeState::Polling {
                location: location.to_string(),
                interval_secs: 0,
            },
        )
        .await
    }

    /// Posts a clarification response to `location`, per "Clarification
    /// Response", then resumes polling.
    pub async fn respond_to_clarification(&self, location: &str, response_text: impl Into<String>) -> ExchangeOutcome {
        let body = ClarificationResponseRequest {
            action: ClarificationAction::ClarificationResponse,
            clarification_response: response_text.into(),
        };
        self.post_clarification_action(location, &body).await
    }

    /// Posts an updated request to `location`, per "Updated Request".
    ///
    /// Validates client-side, before sending, that `new_resource_token`
    /// carries the same `iss`/`agent`/`agent_jkt` as `original_resource_token`
    /// -- the draft requires this invariant, and checking it locally means a
    /// caller mistake never reaches the wire. Returns
    /// [`ExchangeError::UpdatedRequestClaimsMismatch`] without sending
    /// anything if the claims differ.
    pub async fn submit_updated_request(
        &self,
        location: &str,
        original_resource_token: &str,
        new_resource_token: &str,
        justification: Option<String>,
    ) -> ExchangeOutcome {
        if let Err(e) = assert_same_resource_token_identity(original_resource_token, new_resource_token) {
            return ExchangeOutcome::Error(e);
        }
        let body = UpdatedRequest {
            action: ClarificationAction::UpdatedRequest,
            resource_token: new_resource_token.to_string(),
            justification,
        };
        self.post_clarification_action(location, &body).await
    }

    /// DELETEs `location` to cancel the pending session, per "Cancel
    /// Request". The PS terminates the session; subsequent polls MUST map to
    /// `410 Gone`, which the caller observes via a later [`Self::poll`] call.
    pub async fn cancel(&self, location: &str) -> Result<(), ExchangeError> {
        let builder = self.http.delete(location);
        let builder = self.signer.sign(builder, &[]);
        builder.send().await?.error_for_status()?;
        Ok(())
    }

    async fn post_clarification_action(&self, location: &str, body: &impl serde::Serialize) -> ExchangeOutcome {
        let bytes = match Self::encode_request_body(body) {
            Ok(b) => b,
            Err(e) => return ExchangeOutcome::Error(e),
        };
        let builder = self.http.post(location).json(body);
        let builder = self.signer.sign(builder, &bytes);
        let response = match builder.send().await {
            Ok(r) => r,
            Err(e) => return ExchangeOutcome::Error(ExchangeError::Transport(e)),
        };
        self.handle_response(
            response,
            ExchangeState::Polling {
                location: location.to_string(),
                interval_secs: 0,
            },
        )
        .await
    }

    /// Translates one HTTP response into an [`ExchangeEvent`], feeds it to
    /// [`step`], and loops (sleeping per the action's backoff) while the core
    /// asks to keep polling automatically. Stops and surfaces an
    /// [`ExchangeOutcome`] once the core reaches `Stop`.
    async fn handle_response(&self, response: reqwest::Response, state: ExchangeState) -> ExchangeOutcome {
        let mut state = state;
        let mut response = response;
        let mut polls: u32 = 0;
        loop {
            let event = match event_from_response(response).await {
                Ok(e) => e,
                Err(e) => return ExchangeOutcome::Error(e),
            };
            let (new_state, action) = step(state, event);
            state = new_state;
            match action {
                ExchangeAction::Stop => return outcome_from_state(state),
                ExchangeAction::PollAfter { location, after_secs } => {
                    polls += 1;
                    if polls > self.max_poll_iterations {
                        return ExchangeOutcome::Error(ExchangeError::PollBudgetExhausted { polls });
                    }
                    let sleep_secs = after_secs.max(self.min_poll_interval_secs);
                    if sleep_secs > 0 {
                        tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
                    }
                    let builder = self
                        .http
                        .get(&location)
                        .header("Prefer", format!("wait={}", self.wait_secs));
                    let builder = self.signer.sign(builder, &[]);
                    response = match builder.send().await {
                        Ok(r) => r,
                        Err(e) => return ExchangeOutcome::Error(ExchangeError::Transport(e)),
                    };
                }
            }
        }
    }
}

fn outcome_from_state(state: ExchangeState) -> ExchangeOutcome {
    match state {
        ExchangeState::Granted(grant) => ExchangeOutcome::Granted(grant),
        ExchangeState::Interacting {
            location, url, code, ..
        } => ExchangeOutcome::Interaction {
            url,
            code,
            poll_location: location,
        },
        ExchangeState::AwaitingClarification {
            location,
            question,
            timeout,
            options,
            ..
        } => ExchangeOutcome::Clarification {
            question,
            timeout,
            options,
            poll_location: location,
        },
        ExchangeState::Polling { location, .. } => ExchangeOutcome::Pending {
            poll_location: location,
        },
        ExchangeState::Terminal(reason) => outcome_from_terminal(reason),
        // `Stop` is only ever returned alongside one of the states above; the
        // core never asks the driver to stop while still `AwaitingInitial`
        // except on a `503` with no Location yet (see `service_unavailable_transition`),
        // which the caller observes as a transport-level surprise rather than
        // a modeled outcome -- treat it as a server error to retry from scratch,
        // matching "start over" in the state machine diagram.
        ExchangeState::AwaitingInitial => ExchangeOutcome::Error(ExchangeError::UnexpectedStatus(503)),
    }
}

fn outcome_from_terminal(reason: TerminalReason) -> ExchangeOutcome {
    match reason {
        TerminalReason::InvalidRequest(err) | TerminalReason::Unauthorized(err) => {
            match token_endpoint_error_from_code(&err.error) {
                Some(typed) => ExchangeOutcome::Error(ExchangeError::TokenEndpoint(typed)),
                None => ExchangeOutcome::Error(ExchangeError::UnrecognizedTokenEndpointError(Box::new(err))),
            }
        }
        TerminalReason::PaymentRequired { location } => ExchangeOutcome::Pending {
            poll_location: location,
        },
        TerminalReason::Denied(polling_error) => ExchangeOutcome::Denied(TerminalReason::Denied(polling_error)),
        TerminalReason::Expired => ExchangeOutcome::Denied(TerminalReason::Expired),
        TerminalReason::Gone => ExchangeOutcome::Denied(TerminalReason::Gone),
        TerminalReason::ServerError => ExchangeOutcome::Error(ExchangeError::UnexpectedStatus(500)),
    }
}

fn token_endpoint_error_from_code(code: &str) -> Option<TokenEndpointError> {
    match code {
        "invalid_request" => Some(TokenEndpointError::InvalidRequest),
        "invalid_agent_token" => Some(TokenEndpointError::InvalidAgentToken),
        "expired_agent_token" => Some(TokenEndpointError::ExpiredAgentToken),
        "invalid_resource_token" => Some(TokenEndpointError::InvalidResourceToken),
        "expired_resource_token" => Some(TokenEndpointError::ExpiredResourceToken),
        "user_unreachable" => Some(TokenEndpointError::UserUnreachable),
        "server_error" => Some(TokenEndpointError::ServerError),
        _ => None,
    }
}

async fn event_from_response(response: reqwest::Response) -> Result<ExchangeEvent, ExchangeError> {
    let status = response.status().as_u16();
    let location = resolve_location(&response);
    let retry_after = header_u64(&response, "retry-after");
    let requirement_header = header_string(&response, headers::REQUIREMENT)
        .map(|raw| trogon_identity_types::aauth::Requirement::parse(&raw));

    match status {
        200 => {
            let grant: TokenGrantResponse = response.json().await.map_err(ExchangeError::MalformedBody)?;
            Ok(ExchangeEvent::Granted(grant))
        }
        202 => {
            let location = location.ok_or(ExchangeError::MissingLocation)?;
            let retry_after_secs = retry_after.ok_or(ExchangeError::MissingRetryAfter)?;
            let body: PendingResponse = response.json().await.map_err(ExchangeError::MalformedBody)?;
            Ok(ExchangeEvent::Pending {
                headers: PendingHeaders {
                    location,
                    retry_after_secs,
                    requirement: requirement_header,
                },
                body,
            })
        }
        400 => {
            let err: ErrorResponse = response.json().await.map_err(ExchangeError::MalformedBody)?;
            Ok(ExchangeEvent::InvalidRequest(err))
        }
        401 => {
            let err: ErrorResponse = response.json().await.map_err(ExchangeError::MalformedBody)?;
            Ok(ExchangeEvent::Unauthorized(err))
        }
        402 => {
            let location = location.ok_or(ExchangeError::MissingLocation)?;
            let retry_after_secs = retry_after.unwrap_or(0);
            Ok(ExchangeEvent::PaymentRequired(PendingHeaders {
                location,
                retry_after_secs,
                requirement: requirement_header,
            }))
        }
        403 => {
            let err: ErrorResponse = response.json().await.map_err(ExchangeError::MalformedBody)?;
            let polling_error = polling_error_from_code(&err.error).unwrap_or(PollingError::Denied);
            Ok(ExchangeEvent::Denied(polling_error))
        }
        408 => Ok(ExchangeEvent::Expired),
        410 => Ok(ExchangeEvent::Gone),
        429 => Ok(ExchangeEvent::SlowDown),
        500 => Ok(ExchangeEvent::ServerError),
        503 => Ok(ExchangeEvent::ServiceUnavailable {
            retry_after_secs: retry_after.unwrap_or(super::core::DEFAULT_POLL_INTERVAL_SECS),
        }),
        other => Err(ExchangeError::UnexpectedStatus(other)),
    }
}

fn polling_error_from_code(code: &str) -> Option<PollingError> {
    match code {
        "denied" => Some(PollingError::Denied),
        "abandoned" => Some(PollingError::Abandoned),
        "expired" => Some(PollingError::Expired),
        "invalid_code" => Some(PollingError::InvalidCode),
        "slow_down" => Some(PollingError::SlowDown),
        "server_error" => Some(PollingError::ServerError),
        _ => None,
    }
}

fn header_string(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn header_u64(response: &reqwest::Response, name: &str) -> Option<u64> {
    header_string(response, name).and_then(|v| v.parse().ok())
}

/// Resolves the `Location` header against the response's own URL, per HTTP's
/// usual relative-reference resolution (RFC 9110 -- `Location` MAY be
/// relative). A PS returning `/pending/abc123` instead of an absolute URL is
/// still followed correctly, rather than failing the subsequent GET/POST with
/// a "relative URL without base" transport error.
fn resolve_location(response: &reqwest::Response) -> Option<String> {
    let raw = header_string(response, "location")?;
    match response.url().join(&raw) {
        Ok(resolved) => Some(resolved.to_string()),
        Err(_) => Some(raw),
    }
}

/// Decodes a resource token's payload segment without verifying its
/// signature, purely to compare `iss`/`agent`/`agent_jkt` for the "Updated
/// Request" client-side sanity check. The original resource token's
/// signature was already verified when first received; this is not a
/// security boundary.
fn assert_same_resource_token_identity(original: &str, updated: &str) -> Result<(), ExchangeError> {
    let original_claims = decode_resource_claims_unverified(original)?;
    let updated_claims = decode_resource_claims_unverified(updated)?;
    if original_claims.iss == updated_claims.iss
        && original_claims.agent == updated_claims.agent
        && original_claims.agent_jkt == updated_claims.agent_jkt
    {
        Ok(())
    } else {
        Err(ExchangeError::UpdatedRequestClaimsMismatch)
    }
}

#[derive(serde::Deserialize)]
struct ResourceTokenIdentity {
    iss: String,
    agent: String,
    agent_jkt: String,
}

fn decode_resource_claims_unverified(jwt: &str) -> Result<ResourceTokenIdentity, ExchangeError> {
    let mut parts = jwt.splitn(3, '.');
    let _ = parts.next();
    let payload_b64 = parts
        .next()
        .ok_or(ExchangeError::MalformedResourceToken("missing payload segment"))?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64.as_bytes())
        .map_err(|_| ExchangeError::MalformedResourceToken("payload is not valid base64url"))?;
    serde_json::from_slice(&payload).map_err(|_| ExchangeError::MalformedResourceToken("payload is not valid JSON"))
}

#[cfg(test)]
mod tests;
