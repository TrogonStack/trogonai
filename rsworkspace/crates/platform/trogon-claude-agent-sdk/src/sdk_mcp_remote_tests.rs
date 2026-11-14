//! Replays the `bridge` section of `tests/fixtures/sdk_mcp/python_remote.json`: tool schemas with remote `$ref`s
//! called through the pinned Python package's in-process MCP bridge, each call with the answer, the URIs
//! `jsonschema` retrieved and the `DeprecationWarning`s it emitted. Here a scripted retriever serves the documents the
//! recording server served, so the replay checks the retrieval and warning logic without the network.
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "Tests build the JSON-RPC requests the recording sent.")
)]

use super::*;
use crate::{
    McpToolName, McpToolOutput, McpToolRequest, RemoteSchemaError, SdkTool, SdkToolOptions, ToolResultStatus,
    python_jsonschema::NoRetrieval, python_urlopen_text::a2b_base64,
};
use serde_json::json;
use std::{
    collections::{BTreeMap, HashMap},
    io::Read,
    sync::{Mutex, PoisonError},
    thread::ThreadId,
};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

type TestResult = Fallible<()>;

/// The goldens recorded against CPython 3.12.
const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_remote.json");

/// The `tracing` target the warnings Python emits while validating are logged under.
const PYTHON_WARNINGS: &str = "trogon_claude_agent_sdk::python_warnings";

/// The values the fixture's placeholders stand for in this replay, in substitution order. The ports keep the five
/// digits of the recording, so every text that prints a URL keeps its width.
const PLACEHOLDERS: [(&str, &str); 3] = [
    ("{BRIDGE_CLOSED}", "41001"),
    ("{BRIDGE}", "41000"),
    ("{DIR}", "/srv/remote-schemas"),
];

fn substitute(text: &str) -> String {
    PLACEHOLDERS.iter().fold(text.to_owned(), |text, (placeholder, value)| {
        text.replace(placeholder, value)
    })
}

/// [`substitute`] on raw bytes, which need not be text.
fn substitute_bytes(bytes: &[u8]) -> Vec<u8> {
    PLACEHOLDERS.iter().fold(bytes.to_vec(), |bytes, (placeholder, value)| {
        let (placeholder, value) = (placeholder.as_bytes(), value.as_bytes());
        let mut replaced = Vec::with_capacity(bytes.len());
        let mut rest = bytes.as_slice();
        loop {
            if let Some(after) = rest.strip_prefix(placeholder) {
                replaced.extend_from_slice(value);
                rest = after;
            } else if let Some((first, after)) = rest.split_first() {
                replaced.push(*first);
                rest = after;
            } else {
                break;
            }
        }
        replaced
    })
}

fn field<'a>(value: &'a Value, name: &str) -> Fallible<&'a Value> {
    Ok(value
        .get(name)
        .ok_or_else(|| format!("fixture entry without `{name}`: {value}"))?)
}

fn text<'a>(value: &'a Value, name: &str) -> Fallible<&'a str> {
    Ok(field(value, name)?
        .as_str()
        .ok_or_else(|| format!("`{name}` is not text: {value}"))?)
}

fn list<'a>(value: &'a Value, name: &str) -> Fallible<&'a [Value]> {
    Ok(field(value, name)?
        .as_array()
        .ok_or_else(|| format!("`{name}` is not a list: {value}"))?)
}

fn texts(value: &Value, name: &str) -> Fallible<Vec<String>> {
    list(value, name)?
        .iter()
        .map(|item| -> Fallible<String> { Ok(item.as_str().ok_or("an item is not text")?.to_owned()) })
        .collect()
}

/// The recorded bytes: base64, or `[base64, repeat]` chunks.
fn fixture_bytes(recorded: &Value) -> Fallible<Vec<u8>> {
    if let Some(text) = recorded.as_str() {
        return Ok(a2b_base64(text.as_bytes())?);
    }
    let chunks = recorded.as_array().ok_or("bytes are neither base64 nor chunks")?;
    let mut bytes = Vec::new();
    for chunk in chunks {
        let Some([Value::String(text), count]) = chunk.as_array().map(Vec::as_slice) else {
            return Err("a chunk is not [base64, repeat]".into());
        };
        let count = usize::try_from(count.as_u64().ok_or("a repeat is not a count")?)?;
        bytes.extend(a2b_base64(text.as_bytes())?.repeat(count));
    }
    Ok(bytes)
}

fn recorded_error(recorded: &Value) -> Fallible<RemoteSchemaError> {
    Ok(RemoteSchemaError::new(
        text(recorded, "class")?,
        substitute(text(recorded, "text")?),
    ))
}

/// The fixture with its placeholders substituted everywhere but in the base64 bodies.
fn fixture() -> Fallible<Value> {
    Ok(serde_json::from_str(&substitute(FIXTURE))?)
}

/// What the recording server, or `urlopen` itself, did with a URI.
enum Document {
    Body(Vec<u8>),
    /// What `urlopen` raised.
    OpenError(RemoteSchemaError),
    /// What `response.read()` raised once the response was open.
    ReadError(RemoteSchemaError),
}

impl Document {
    fn recorded(recorded: &Value) -> Fallible<Self> {
        if let Some(body) = recorded.get("body") {
            return Ok(Self::Body(substitute_bytes(&fixture_bytes(body)?)));
        }
        if let Some(error) = recorded.get("open_error") {
            return Ok(Self::OpenError(recorded_error(error)?));
        }
        Ok(Self::ReadError(recorded_error(field(recorded, "read_error")?)?))
    }
}

/// A body whose first read fails.
struct FailingRead(RemoteSchemaError);

impl Read for FailingRead {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other(self.0.clone()))
    }
}

/// Answers each URI with its recorded document and records every URI opened.
#[derive(Default)]
struct ScriptedRetriever {
    documents: HashMap<String, Document>,
    opened: Mutex<Vec<String>>,
}

impl ScriptedRetriever {
    fn recorded(documents: &Value) -> Fallible<Self> {
        let documents = documents
            .as_object()
            .ok_or("documents is not an object")?
            .iter()
            .map(|(uri, recorded)| -> Fallible<(String, Document)> { Ok((uri.clone(), Document::recorded(recorded)?)) })
            .collect::<Fallible<HashMap<_, _>>>()?;
        Ok(Self {
            documents,
            opened: Mutex::default(),
        })
    }

    /// The URIs opened since the last call.
    fn take_opened(&self) -> Vec<String> {
        std::mem::take(&mut *self.opened.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl RemoteSchemaRetriever for ScriptedRetriever {
    fn open(&self, uri: &str) -> Result<Box<dyn Read + '_>, RemoteSchemaError> {
        self.opened
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(uri.to_owned());
        match self.documents.get(uri) {
            Some(Document::Body(body)) => Ok(Box::new(body.as_slice())),
            Some(Document::OpenError(error)) => Err(error.clone()),
            Some(Document::ReadError(error)) => Ok(Box::new(FailingRead(error.clone()))),
            None => Err(RemoteSchemaError::new(
                "URLError",
                format!("nothing recorded for {uri}"),
            )),
        }
    }
}

/// The fields of a `tracing` event, as text.
#[derive(Default)]
struct Fields(BTreeMap<&'static str, String>);

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name(), value.to_owned());
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0.insert(field.name(), value.to_string());
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0.insert(field.name(), value.to_string());
    }
}

/// An event a [`Capture`] saw, with the thread that emitted it.
struct Captured {
    target: String,
    level: tracing::Level,
    thread: ThreadId,
    fields: BTreeMap<&'static str, String>,
}

/// Keeps every event, from any thread whose default dispatcher it is.
struct Capture(Arc<Mutex<Vec<Captured>>>);

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let metadata = event.metadata();
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(Captured {
            target: metadata.target().to_owned(),
            level: *metadata.level(),
            thread: std::thread::current().id(),
            fields: fields.0,
        });
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// The Python warnings captured since the last call, leaving out any other event.
fn take_warnings(events: &Mutex<Vec<Captured>>) -> Vec<Captured> {
    std::mem::take(&mut *events.lock().unwrap_or_else(PoisonError::into_inner))
        .into_iter()
        .filter(|event| event.target == PYTHON_WARNINGS)
        .collect()
}

/// A Python dialect server with one tool, `t`, whose handler answers `x`, declaring `schema`.
fn server(schema: &Value) -> Fallible<LocalMcpServer> {
    let tool = SdkTool::from(SdkToolOptions {
        name: McpToolName::from("t"),
        description: String::new(),
        input_schema: McpToolInputSchema::new(schema.as_object().ok_or("schema is not an object")?.clone()),
        handler: Arc::new(|_: McpToolRequest| async move {
            Ok(McpToolOutput {
                status: ToolResultStatus::NotReported,
                ..McpToolOutput::text("x")
            })
        }),
        annotations: None,
        search_hint: None,
        always_load: false,
    });
    Ok(LocalMcpServer::from(LocalMcpServerOptions {
        tools: Some(tool.into()),
        dialect: McpServerDialect::Python,
        ..LocalMcpServerOptions::new(McpServerName::from("s"))
    }))
}

fn object(value: Value) -> Fallible<JsonObject> {
    Ok(serde_json::from_value(value)?)
}

async fn initialized(server: &LocalMcpServer) -> Fallible<Arc<dyn SdkMcpSession>> {
    let session = server.open_session();
    let initialize = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}
    }});
    session
        .request(object(initialize)?, CancellationToken::new())
        .await?
        .ok_or("missing initialize response")?;
    let notice = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    assert!(
        session
            .request(object(notice)?, CancellationToken::new())
            .await?
            .is_none()
    );
    Ok(session)
}

/// Calls `t` with `arguments`: `OK`, `ERR` followed by the text of the error result, or `RPCERR` followed by the
/// JSON-RPC error, as the fixture records answers.
async fn call(session: &Arc<dyn SdkMcpSession>, arguments: &Value) -> Fallible<String> {
    let call = object(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
        "name":"t","arguments":arguments
    }}))?;
    let response = session
        .request(call, CancellationToken::new())
        .await?
        .ok_or("missing tools/call response")?;
    if let Some(error) = response.get("error") {
        return Ok(format!("RPCERR {error}"));
    }
    let result = response.get("result").ok_or("tools/call without a result")?;
    if result.get("isError").and_then(Value::as_bool) != Some(true) {
        return Ok("OK".to_owned());
    }
    let message = result
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .ok_or("error result without text")?;
    Ok(format!("ERR {message}"))
}

/// Every call of the bridge section answers as Python answered, opens the URIs Python retrieved in the same order
/// and logs a `DeprecationWarning` with the fields of the one Python emitted for each response that opened, on the
/// validation thread and under the caller's subscriber.
#[tokio::test]
async fn python_dialect_retrieves_remote_references_and_warns_like_the_python_package() -> TestResult {
    let fixture = fixture()?;
    let warning = field(&fixture, "warning")?;
    let bridge = field(&fixture, "bridge")?;
    let retriever = Arc::new(ScriptedRetriever::recorded(field(bridge, "documents")?)?);
    let events = Arc::new(Mutex::new(Vec::new()));
    let _capture = tracing::subscriber::set_default(Capture(Arc::clone(&events)));
    let caller = std::thread::current().id();
    let cases = list(bridge, "cases")?;
    let (mut calls, mut warned) = (0, 0);
    for case in cases {
        let name = text(case, "name")?;
        let schema = field(case, "schema")?;
        let server = server(schema)?.with_remote_schema_retriever(Arc::<ScriptedRetriever>::clone(&retriever));
        let session = initialized(&server).await?;
        for recorded in list(case, "calls")? {
            let arguments = field(recorded, "arguments")?;
            let context = format!("case {name}: schema {schema} arguments {arguments}");
            assert_eq!(call(&session, arguments).await?, text(recorded, "answer")?, "{context}");
            let retrievals = texts(recorded, "retrievals")?;
            assert_eq!(retriever.take_opened(), retrievals, "{context}");
            let opened: Vec<&String> = retrievals
                .iter()
                .filter(|uri| !matches!(retriever.documents.get(uri.as_str()), Some(Document::OpenError(_))))
                .collect();
            let messages = texts(recorded, "warnings")?;
            let warnings = take_warnings(&events);
            assert_eq!(warnings.len(), messages.len(), "{context}");
            assert_eq!(warnings.len(), opened.len(), "{context}");
            for ((event, message), uri) in warnings.iter().zip(&messages).zip(opened) {
                assert_eq!(event.level, tracing::Level::WARN, "{context}");
                assert_ne!(event.thread, caller, "{context}");
                let fields = |key: &str| event.fields.get(key).map(String::as_str);
                assert_eq!(fields("category"), Some(text(warning, "category")?), "{context}");
                assert_eq!(fields("message"), Some(message.as_str()), "{context}");
                assert_eq!(fields("filename"), Some(text(warning, "filename")?), "{context}");
                let lineno = field(warning, "lineno")?.to_string();
                assert_eq!(fields("lineno"), Some(lineno.as_str()), "{context}");
                let source = Some("jsonschema.validators._warn_for_remote_retrieve");
                assert_eq!(fields("source"), source, "{context}");
                assert_eq!(fields("uri"), Some(uri.as_str()), "{context}");
                warned += 1;
            }
            calls += 1;
        }
    }
    assert_eq!(cases.len(), 110);
    assert_eq!(calls, 235);
    assert_eq!(warned, 234);
    Ok(())
}

/// A server retrieves with `UrlopenRetriever` unless told otherwise; a `data:` URL needs no network.
#[tokio::test]
async fn python_dialect_retrieves_with_urlopen_by_default() -> TestResult {
    let uri = "data:application/json,%7B%22type%22%3A%22integer%22%7D";
    let schema = json!({"type": "object", "properties": {"a": {"$ref": uri}}});
    let session = initialized(&server(&schema)?).await?;
    let events = Arc::new(Mutex::new(Vec::new()));
    let _capture = tracing::subscriber::set_default(Capture(Arc::clone(&events)));
    assert_eq!(call(&session, &json!({"a": 1})).await?, "OK");
    let expected = "ERR Input validation error: 'x' is not of type 'integer'";
    assert_eq!(call(&session, &json!({"a": "x"})).await?, expected);
    assert_eq!(call(&session, &json!({})).await?, "OK");
    assert_eq!(take_warnings(&events).len(), 2);
    Ok(())
}

/// `with_remote_schema_retriever` changes what later sessions retrieve with; a session already open keeps its own.
#[tokio::test]
async fn sessions_keep_the_retriever_their_server_had_when_they_opened() -> TestResult {
    let uri = "http://127.0.0.1:41000/int.json";
    let documents = json!({uri: {"body": "eyJ0eXBlIjogImludGVnZXIifQ=="}});
    let first = Arc::new(ScriptedRetriever::recorded(&documents)?);
    let second = Arc::new(ScriptedRetriever::recorded(&documents)?);
    let schema = json!({"type": "object", "properties": {"a": {"$ref": uri}}});
    let server = server(&schema)?.with_remote_schema_retriever(Arc::<ScriptedRetriever>::clone(&first));
    let early = initialized(&server).await?;
    let server = server.with_remote_schema_retriever(Arc::<ScriptedRetriever>::clone(&second));
    let late = initialized(&server).await?;
    let expected = "ERR Input validation error: 'x' is not of type 'integer'";
    assert_eq!(call(&early, &json!({"a": "x"})).await?, expected);
    assert_eq!(first.take_opened(), [uri]);
    assert!(second.take_opened().is_empty());
    assert_eq!(call(&late, &json!({"a": "x"})).await?, expected);
    assert!(first.take_opened().is_empty());
    assert_eq!(second.take_opened(), [uri]);
    Ok(())
}

/// A scripted retriever answers what it has no recording of with an error, which leaves the reference
/// `Unresolvable` like any other failed retrieval.
#[test]
fn the_scripted_retriever_refuses_what_it_has_no_recording_of() -> TestResult {
    let retriever = ScriptedRetriever::default();
    let refused = retriever.open("http://127.0.0.1:41000/none.json").err();
    let text = "nothing recorded for http://127.0.0.1:41000/none.json";
    assert_eq!(refused, Some(RemoteSchemaError::new("URLError", text)));
    assert_eq!(retriever.take_opened(), ["http://127.0.0.1:41000/none.json"]);
    Ok(())
}

/// `NoRetrieval` opens nothing, as a registry without `retrieve` raises `NoSuchResource`: the reference is
/// `Unresolvable` and nothing warns.
#[tokio::test]
async fn no_retrieval_leaves_every_remote_reference_unresolvable_without_a_warning() -> TestResult {
    let uri = "http://127.0.0.1:41000/int.json";
    assert_eq!(
        NoRetrieval.open(uri).err(),
        Some(RemoteSchemaError::new("NoSuchResource", uri))
    );
    let schema = json!({"type": "object", "properties": {"a": {"$ref": uri}}});
    let server = server(&schema)?.with_remote_schema_retriever(Arc::new(NoRetrieval));
    let session = initialized(&server).await?;
    let events = Arc::new(Mutex::new(Vec::new()));
    let _capture = tracing::subscriber::set_default(Capture(Arc::clone(&events)));
    assert_eq!(
        call(&session, &json!({"a": 1})).await?,
        format!("ERR Unresolvable: {uri}")
    );
    assert!(take_warnings(&events).is_empty());
    Ok(())
}
