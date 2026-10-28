use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use axum::body::{Body, to_bytes};
use axum::http::Request;
use bytes::Bytes;
use serde::Deserialize;
use tower::ServiceExt;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimBucket, ClaimBucketBinding, ClaimCheckPublisher, MaxPayload, MockJetStreamPublisher, MockObjectStore,
    StreamMaxAge,
};
use trogon_std::NonZeroDuration;
use trogon_std::time::FixedEpochClock;

use super::ack_budget::AckBudget;
use super::config::{
    SlackAppToken, SlackConfig, SlackSigningSecret, SlackSocketModeConfig, SlackTransportConfig, SlackWebhookConfig,
};
use super::server::{SlackBridge, router_with_clock};
use super::socket_mode::handle_text_frame;

const REQUEST_NOW_SECONDS: u64 = 1_700_000_000;
const SIGNING_SECRET: &str = "bolt-conformance-signing-secret";
const SUBJECT_PREFIX: &str = "slack";
const UNROUTABLE_SUBJECT: &str = "slack.unroutable";
const DEDUP_HEADER: &str = "Nats-Msg-Id";
const HEADER_ENVELOPE_ID: &str = "X-Slack-Envelope-Id";
const HEADER_REQUEST_TIMESTAMP: &str = "X-Slack-Request-Timestamp";
const HEADERS_EVENT_METADATA: [&str; 2] = ["X-Slack-Event-Subtype", "X-Slack-Channel-Type"];
const PAYLOAD_PREVIEW_CHARS: usize = 96;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(transparent)]
struct FixtureId(String);

impl fmt::Display for FixtureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
enum GapId {
    G1,
    G2,
    G3,
    G4,
    G6,
    G8,
    G9,
    G10,
    G11,
    G12,
    G13,
    G14,
    G15,
    G16,
    G18,
    G19,
    G20,
    G21,
    G22,
    G23,
}

impl fmt::Display for GapId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Transport {
    Http,
    SocketMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PayloadKind {
    Event,
    Interaction,
    Command,
    SslCheck,
    UrlVerification,
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Publish,
    Inline,
    Reject,
    Ignore,
}

#[derive(Debug, Clone, Deserialize)]
struct BodyFill {
    prefix: String,
    filler: String,
    count: usize,
    suffix: String,
}

impl BodyFill {
    fn expand(&self) -> String {
        format!("{}{}{}", self.prefix, self.filler.repeat(self.count), self.suffix)
    }
}

#[derive(Debug, Deserialize)]
struct Provenance {
    bolt_version: String,
    bolt_commit: String,
    payload_source: String,
}

#[derive(Debug, Deserialize)]
struct HttpInput {
    headers: BTreeMap<String, String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    body_fill: Option<BodyFill>,
}

impl HttpInput {
    fn body(&self) -> Vec<u8> {
        match (&self.body, &self.body_fill) {
            (Some(body), None) => body.clone().into_bytes(),
            (None, Some(fill)) => fill.expand().into_bytes(),
            _ => panic!("an HTTP fixture carries exactly one of body and body_fill"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SocketInput {
    frame: String,
}

#[derive(Debug, Deserialize)]
struct ExpectedNats {
    subject_suffix: String,
    dedup_id: Option<String>,
    payload: String,
    payload_fill: Option<BodyFill>,
    headers: BTreeMap<String, String>,
}

impl ExpectedNats {
    fn payload_bytes(&self) -> Vec<u8> {
        match &self.payload_fill {
            Some(fill) => fill.expand().into_bytes(),
            None => self.payload.clone().into_bytes(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ExpectedHttp {
    status: u16,
    body: Option<String>,
    challenge: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedSocketAck {
    envelope_id: String,
    payload: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct Expected {
    outcome: Outcome,
    nats: Option<ExpectedNats>,
    http: Option<ExpectedHttp>,
    socket_ack: Option<ExpectedSocketAck>,
}

#[derive(Debug, Deserialize)]
struct Fixture {
    id: FixtureId,
    kind: PayloadKind,
    transport: Transport,
    provenance: Provenance,
    not_representable: Option<String>,
    input: serde_json::Value,
    expected: Expected,
}

impl Fixture {
    fn http_input(&self) -> HttpInput {
        serde_json::from_value(self.input.clone()).unwrap_or_else(|e| panic!("{}: bad HTTP input: {e}", self.id))
    }

    fn socket_input(&self) -> SocketInput {
        serde_json::from_value(self.input.clone()).unwrap_or_else(|e| panic!("{}: bad socket input: {e}", self.id))
    }

    fn expects_ack_body(&self) -> bool {
        let http_body = self.expected.http.as_ref().is_some_and(|http| http.body.is_some());
        let socket_payload = self
            .expected
            .socket_ack
            .as_ref()
            .is_some_and(|ack| ack.payload.is_some());
        http_body || socket_payload
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NotRepresentableMarker {
    NotRepresentable,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LedgerEntry {
    NotRepresentable(NotRepresentableMarker),
    Gaps(BTreeSet<GapId>),
}

#[derive(Debug, Deserialize)]
struct Ledger {
    schema_version: u32,
    entries: BTreeMap<FixtureId, LedgerEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Field {
    Outcome,
    Status,
    ResponseBody,
    Deliveries,
    Subject,
    DedupId,
    Payload,
    PayloadReserialized,
    MissingHeader(String),
    UnexpectedHeader(String),
    HeaderValue(String),
    SocketAck,
    Unroutable,
}

#[derive(Debug, Clone)]
struct Diff {
    field: Field,
    expected: String,
    actual: String,
}

impl fmt::Display for Diff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: expected {}, got {}", self.field, self.expected, self.actual)
    }
}

impl GapId {
    fn explains(self, fixture: &Fixture, diff: &Diff) -> bool {
        let missing = match &diff.field {
            Field::MissingHeader(name) => Some(name.as_str()),
            _ => None,
        };
        match self {
            GapId::G2 => fixture.expects_ack_body() && matches!(diff.field, Field::ResponseBody | Field::SocketAck),
            GapId::G11 => fixture.transport == Transport::SocketMode && diff.field == Field::PayloadReserialized,
            GapId::G13 => missing.is_some_and(|name| name == HEADER_ENVELOPE_ID || name == HEADER_REQUEST_TIMESTAMP),
            GapId::G22 => missing.is_some_and(|name| HEADERS_EVENT_METADATA.contains(&name)),
            GapId::G23 => diff.field == Field::Unroutable && fixture.expected.outcome == Outcome::Reject,
            _ => false,
        }
    }
}

#[derive(Debug)]
struct Delivery {
    subject: String,
    headers: BTreeMap<String, String>,
    payload: Bytes,
}

#[derive(Debug)]
struct Published {
    routed: Vec<Delivery>,
    unroutable: usize,
}

#[derive(Debug)]
struct Observed {
    outcome: Outcome,
    deliveries: Vec<Delivery>,
    unroutable: usize,
    status: Option<u16>,
    body: Option<Bytes>,
    socket_ack: Option<String>,
    failure: Option<String>,
}

fn fixtures_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/slack/bolt")
}

fn load_fixtures() -> Vec<Fixture> {
    let directory = fixtures_directory();
    let mut fixtures = Vec::new();
    let entries = std::fs::read_dir(&directory).unwrap_or_else(|e| panic!("cannot read {}: {e}", directory.display()));
    for entry in entries {
        let path = entry.expect("readable directory entry").path();
        let is_fixture = path.extension().is_some_and(|ext| ext == "json")
            && path.file_name().is_some_and(|name| name != "ledger.json");
        if !is_fixture {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let fixture: Fixture =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()));
        fixtures.push(fixture);
    }
    fixtures.sort_by(|a, b| a.id.cmp(&b.id));
    fixtures
}

fn load_ledger() -> Ledger {
    let path = fixtures_directory().join("ledger.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()))
}

fn wrap_publisher(publisher: MockJetStreamPublisher) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
    ClaimCheckPublisher::new(
        publisher,
        ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
        MaxPayload::from_server_limit(usize::MAX),
    )
}

fn gateway_config(transport: SlackTransportConfig) -> SlackConfig {
    SlackConfig {
        subject_prefix: NatsToken::new(SUBJECT_PREFIX).expect("valid token"),
        stream_name: NatsToken::new("SLACK").expect("valid token"),
        stream_max_age: StreamMaxAge::from_secs(3600).expect("valid max age"),
        ack_budget: AckBudget::default(),
        transport,
    }
}

fn webhook_config() -> SlackConfig {
    gateway_config(SlackTransportConfig::Webhook(SlackWebhookConfig {
        signing_secret: SlackSigningSecret::new(SIGNING_SECRET).expect("valid secret"),
        timestamp_max_drift: NonZeroDuration::from_secs(300).expect("valid drift"),
        max_body_size: super::constants::DEFAULT_HTTP_BODY_SIZE_MAX,
    }))
}

fn socket_mode_config() -> SlackConfig {
    gateway_config(SlackTransportConfig::SocketMode(SlackSocketModeConfig {
        app_token: SlackAppToken::new("xapp-conformance").expect("valid token"),
    }))
}

fn published(publisher: &MockJetStreamPublisher) -> Published {
    let messages = publisher.published_messages();
    let unroutable = messages
        .iter()
        .filter(|message| message.subject == UNROUTABLE_SUBJECT)
        .count();
    let routed = messages
        .into_iter()
        .filter(|message| message.subject != UNROUTABLE_SUBJECT)
        .map(|message| Delivery {
            subject: message.subject,
            headers: message
                .headers
                .iter()
                .filter_map(|(name, values)| {
                    values
                        .first()
                        .map(|value| (name.to_string(), value.as_str().to_owned()))
                })
                .collect(),
            payload: message.payload,
        })
        .collect();
    Published { routed, unroutable }
}

async fn observe_http(fixture: &Fixture) -> Observed {
    let input = fixture.http_input();
    let publisher = MockJetStreamPublisher::new();
    let config = webhook_config();
    let app = router_with_clock(
        wrap_publisher(publisher.clone()),
        &config,
        config.webhook().expect("webhook transport"),
        FixedEpochClock::from_secs(REQUEST_NOW_SECONDS),
    );

    let mut builder = Request::builder().method("POST").uri("/webhook");
    for (name, value) in &input.headers {
        builder = builder.header(name, value);
    }
    let request = builder.body(Body::from(input.body())).expect("valid request");
    let response = app.oneshot(request).await.expect("router is infallible");

    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("readable response body");
    let published = published(&publisher);
    let outcome = if !published.routed.is_empty() {
        Outcome::Publish
    } else if status.is_success() {
        Outcome::Inline
    } else {
        Outcome::Reject
    };
    Observed {
        outcome,
        deliveries: published.routed,
        unroutable: published.unroutable,
        status: Some(status.as_u16()),
        body: Some(body),
        socket_ack: None,
        failure: None,
    }
}

async fn observe_socket(fixture: &Fixture) -> Observed {
    let input = fixture.socket_input();
    let publisher = MockJetStreamPublisher::new();
    let bridge = SlackBridge::new(wrap_publisher(publisher.clone()), &socket_mode_config());

    let (socket_ack, failure) = match handle_text_frame(&bridge, &input.frame).await {
        Ok((_, ack)) => (ack, None),
        Err(error) => (None, Some(error.to_string())),
    };
    let published = published(&publisher);
    let outcome = if published.routed.is_empty() {
        Outcome::Reject
    } else {
        Outcome::Publish
    };
    Observed {
        outcome,
        deliveries: published.routed,
        unroutable: published.unroutable,
        status: None,
        body: None,
        socket_ack,
        failure,
    }
}

async fn observe(fixture: &Fixture) -> Observed {
    match fixture.transport {
        Transport::Http => observe_http(fixture).await,
        Transport::SocketMode => observe_socket(fixture).await,
    }
}

fn describe_bytes(bytes: &[u8], other: &[u8]) -> String {
    let offset = bytes.iter().zip(other).position(|(a, b)| a != b);
    let preview: String = String::from_utf8_lossy(bytes)
        .chars()
        .take(PAYLOAD_PREVIEW_CHARS)
        .collect();
    match offset {
        Some(offset) => format!(
            "{} bytes, first difference at byte {offset}, starts {preview:?}",
            bytes.len()
        ),
        None => format!("{} bytes, starts {preview:?}", bytes.len()),
    }
}

fn normalized_ack_payload(ack: &serde_json::Value) -> Option<serde_json::Value> {
    match ack.get("payload") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Object(map)) if map.is_empty() => None,
        Some(other) => Some(other.clone()),
    }
}

fn compare_http(expected: &Expected, observed: &Observed, diffs: &mut Vec<Diff>) {
    let Some(http) = &expected.http else { return };
    if let Some(status) = observed.status
        && status != http.status
    {
        diffs.push(Diff {
            field: Field::Status,
            expected: http.status.to_string(),
            actual: status.to_string(),
        });
    }
    let body = observed.body.as_deref().unwrap_or_default();
    let body_text = String::from_utf8_lossy(body);
    if let Some(challenge) = &http.challenge {
        if !answers_challenge(&body_text, challenge) {
            diffs.push(Diff {
                field: Field::ResponseBody,
                expected: format!("a url_verification response carrying exactly {challenge:?}"),
                actual: format!("{body_text:?}"),
            });
        }
    } else if let Some(expected_body) = &http.body {
        if body_text != expected_body.as_str() {
            diffs.push(Diff {
                field: Field::ResponseBody,
                expected: format!("{expected_body:?}"),
                actual: format!("{body_text:?}"),
            });
        }
    } else if expected.outcome == Outcome::Publish && !body.is_empty() {
        diffs.push(Diff {
            field: Field::ResponseBody,
            expected: "empty".to_owned(),
            actual: format!("{body_text:?}"),
        });
    }
}

fn answers_challenge(body: &str, challenge: &str) -> bool {
    if body == challenge {
        return true;
    }
    if let Ok(serde_json::Value::Object(fields)) = serde_json::from_str::<serde_json::Value>(body) {
        return fields.len() == 1 && fields.get("challenge").and_then(serde_json::Value::as_str) == Some(challenge);
    }
    let mut fields: Vec<(String, String)> = form_urlencoded::parse(body.as_bytes()).into_owned().collect();
    matches!(fields.pop(), Some((name, value)) if fields.is_empty() && name == "challenge" && value == challenge)
}

fn compare_socket_ack(expected: &Expected, observed: &Observed, diffs: &mut Vec<Diff>) {
    let Some(ack) = &expected.socket_ack else { return };
    let expected_text = format!("envelope {} payload {:?}", ack.envelope_id, ack.payload);
    let Some(frame) = &observed.socket_ack else {
        diffs.push(Diff {
            field: Field::SocketAck,
            expected: expected_text,
            actual: "no acknowledgement frame".to_owned(),
        });
        return;
    };
    let parsed: serde_json::Value = serde_json::from_str(frame).unwrap_or(serde_json::Value::Null);
    let envelope_id = parsed.get("envelope_id").and_then(|value| value.as_str());
    let payload = normalized_ack_payload(&parsed);
    if envelope_id != Some(ack.envelope_id.as_str()) || payload != ack.payload {
        diffs.push(Diff {
            field: Field::SocketAck,
            expected: expected_text,
            actual: format!("envelope {envelope_id:?} payload {payload:?}"),
        });
    }
}

fn compare_delivery(nats: &ExpectedNats, delivery: &Delivery, diffs: &mut Vec<Diff>) {
    let subject = format!("{SUBJECT_PREFIX}.{}", nats.subject_suffix);
    if delivery.subject != subject {
        diffs.push(Diff {
            field: Field::Subject,
            expected: subject,
            actual: delivery.subject.clone(),
        });
    }

    let dedup = delivery.headers.get(DEDUP_HEADER);
    if dedup != nats.dedup_id.as_ref() {
        diffs.push(Diff {
            field: Field::DedupId,
            expected: format!("{:?}", nats.dedup_id),
            actual: format!("{dedup:?}"),
        });
    }

    let expected_payload = nats.payload_bytes();
    if delivery.payload.as_ref() != expected_payload.as_slice() {
        let same_value = matches!(
            (
                serde_json::from_slice::<serde_json::Value>(&expected_payload),
                serde_json::from_slice::<serde_json::Value>(&delivery.payload),
            ),
            (Ok(expected_value), Ok(actual_value)) if expected_value == actual_value
        );
        diffs.push(Diff {
            field: if same_value {
                Field::PayloadReserialized
            } else {
                Field::Payload
            },
            expected: describe_bytes(&expected_payload, &delivery.payload),
            actual: describe_bytes(&delivery.payload, &expected_payload),
        });
    }

    for (name, value) in &nats.headers {
        match delivery.headers.get(name) {
            Some(actual) if actual == value => {}
            Some(actual) => diffs.push(Diff {
                field: Field::HeaderValue(name.clone()),
                expected: format!("{value:?}"),
                actual: format!("{actual:?}"),
            }),
            None => diffs.push(Diff {
                field: Field::MissingHeader(name.clone()),
                expected: format!("{value:?}"),
                actual: "missing".to_owned(),
            }),
        }
    }
    for (name, value) in &delivery.headers {
        if name != DEDUP_HEADER && !nats.headers.contains_key(name) {
            diffs.push(Diff {
                field: Field::UnexpectedHeader(name.clone()),
                expected: "absent".to_owned(),
                actual: format!("{value:?}"),
            });
        }
    }
}

fn compare(fixture: &Fixture, observed: &Observed) -> Vec<Diff> {
    let expected = &fixture.expected;
    let mut diffs = Vec::new();

    if observed.outcome != expected.outcome {
        let failure = observed
            .failure
            .as_ref()
            .map(|failure| format!(" ({failure})"))
            .unwrap_or_default();
        diffs.push(Diff {
            field: Field::Outcome,
            expected: format!("{:?}", expected.outcome),
            actual: format!("{:?}{failure}", observed.outcome),
        });
    }

    compare_http(expected, observed, &mut diffs);
    compare_socket_ack(expected, observed, &mut diffs);

    if observed.unroutable > 0 {
        diffs.push(Diff {
            field: Field::Unroutable,
            expected: "0".to_owned(),
            actual: observed.unroutable.to_string(),
        });
    }

    if let Some(nats) = &expected.nats {
        if observed.deliveries.len() > 1 {
            diffs.push(Diff {
                field: Field::Deliveries,
                expected: "1".to_owned(),
                actual: observed.deliveries.len().to_string(),
            });
        }
        if let Some(delivery) = observed.deliveries.first() {
            compare_delivery(nats, delivery, &mut diffs);
        }
    } else if !observed.deliveries.is_empty() {
        diffs.push(Diff {
            field: Field::Deliveries,
            expected: "0".to_owned(),
            actual: observed.deliveries.len().to_string(),
        });
    }

    diffs
}

fn judge(fixture: &Fixture, entry: Option<&LedgerEntry>, diffs: &[Diff]) -> Vec<String> {
    let id = &fixture.id;
    let mut problems = Vec::new();

    if fixture.not_representable.is_some() {
        if !matches!(entry, Some(LedgerEntry::NotRepresentable(_))) {
            problems.push(format!(
                "{id}: not representable, the ledger must say not_representable"
            ));
        }
        return problems;
    }

    let gaps = match entry {
        Some(LedgerEntry::NotRepresentable(_)) => {
            problems.push(format!(
                "{id}: ledgered as not_representable but the fixture is representable"
            ));
            return problems;
        }
        Some(LedgerEntry::Gaps(gaps)) => gaps.clone(),
        None => BTreeSet::new(),
    };

    for diff in diffs {
        if !gaps.iter().any(|gap| gap.explains(fixture, diff)) {
            let listed: Vec<String> = gaps.iter().map(ToString::to_string).collect();
            problems.push(format!("{id}: mismatch not covered by the ledger {listed:?}: {diff}"));
        }
    }
    for gap in &gaps {
        if !diffs.iter().any(|diff| gap.explains(fixture, diff)) {
            problems.push(format!(
                "{id}: stale ledger entry {gap}, it explains no remaining mismatch"
            ));
        }
    }
    problems
}

fn verify_ledger_membership(fixtures: &[Fixture], ledger: &Ledger) -> Vec<String> {
    let known: BTreeSet<&FixtureId> = fixtures.iter().map(|fixture| &fixture.id).collect();
    ledger
        .entries
        .keys()
        .filter(|id| !known.contains(id))
        .map(|id| format!("{id}: ledger entry without a fixture"))
        .collect()
}

#[tokio::test]
async fn gateway_matches_bolt_except_for_ledgered_gaps() {
    let fixtures = load_fixtures();
    let ledger = load_ledger();
    assert_eq!(ledger.schema_version, 1);

    let mut problems = verify_ledger_membership(&fixtures, &ledger);
    for fixture in &fixtures {
        let diffs = if fixture.not_representable.is_some() {
            Vec::new()
        } else {
            compare(fixture, &observe(fixture).await)
        };
        problems.extend(judge(fixture, ledger.entries.get(&fixture.id), &diffs));
    }

    assert!(
        problems.is_empty(),
        "{} conformance problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn every_fixture_names_its_file_and_records_where_it_came_from() {
    let fixtures = load_fixtures();
    assert!(!fixtures.is_empty(), "no Bolt fixtures found");
    for fixture in &fixtures {
        let file = fixtures_directory().join(format!("{}.json", fixture.id));
        assert!(file.exists(), "{}: file name does not match the id", fixture.id);
        assert!(
            !fixture.provenance.bolt_version.is_empty(),
            "{}: no Bolt version",
            fixture.id
        );
        assert!(
            fixture.provenance.bolt_commit.len() == 40
                && fixture
                    .provenance
                    .bolt_commit
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()),
            "{}: Bolt commit is not a full SHA",
            fixture.id
        );
        assert!(
            !fixture.provenance.payload_source.is_empty(),
            "{}: no payload source",
            fixture.id
        );
    }
}

#[test]
fn every_transport_and_payload_kind_is_covered() {
    let fixtures = load_fixtures();
    for transport in [Transport::Http, Transport::SocketMode] {
        for kind in [PayloadKind::Event, PayloadKind::Interaction, PayloadKind::Command] {
            assert!(
                fixtures
                    .iter()
                    .any(|fixture| fixture.transport == transport && fixture.kind == kind),
                "no fixture for {transport:?} {kind:?}"
            );
        }
    }
    for kind in [
        PayloadKind::SslCheck,
        PayloadKind::UrlVerification,
        PayloadKind::Malformed,
    ] {
        assert!(
            fixtures.iter().any(|fixture| fixture.kind == kind),
            "no fixture for {kind:?}"
        );
    }
}

#[cfg(test)]
mod tests;
