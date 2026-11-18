#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "The close test builds the schemas and JSON-RPC requests the recording sent."
    )
)]

use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use trogon_claude_agent_sdk::{
    CallbackError, CancellationToken, JsonObject, LocalMcpServer, LocalMcpServerOptions, McpServerDialect,
    McpServerName, McpToolInputSchema, McpToolName, McpToolOutput, McpToolRequest, RemoteSchemaError,
    RemoteSchemaRetriever, SdkMcpServer, SdkMcpSession, SdkTool, SdkToolOptions, UrlopenRetriever,
    UrlopenRetrieverOptions,
};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

type TestResult = Fallible<()>;

/// For each remote `$ref` the pinned Python bridge retrieved from a server that keeps its end open, the end of every
/// connection as the server saw it when the bridge wrote the tool result, recorded against CPython 3.12 with the
/// garbage collector on and off.
const CLOSE_FIXTURE: &str = include_str!("fixtures/sdk_mcp/python_close.json");

/// How long the server end of a connection waits for the client's `FIN` or `RST`, as the recording waited.
const CLOSE_WAIT: Duration = Duration::from_millis(300);

const SCHEMA: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 19\r\n\r\n{\"type\": \"integer\"}";

const NOT_FOUND: &[u8] = b"HTTP/1.1 404 Not Found\r\nContent-Length: 4\r\n\r\nnope";

/// Answers `count` connections in turn, `GET /schema.json` with a schema and anything else with a 404, and returns
/// the request head of each.
fn serve(listener: TcpListener, count: usize) -> JoinHandle<std::io::Result<Vec<String>>> {
    thread::spawn(move || {
        let mut heads = Vec::new();
        for _ in 0..count {
            let (mut stream, _) = listener.accept()?;
            let mut head = Vec::new();
            let mut byte = [0; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte)? == 1 {
                head.extend_from_slice(&byte);
            }
            let answer = if head.starts_with(b"GET /schema.json ") {
                SCHEMA
            } else {
                NOT_FOUND
            };
            stream.write_all(answer)?;
            heads.push(String::from_utf8_lossy(&head).into_owned());
        }
        Ok(heads)
    })
}

fn request_head(port: u16, path: &str) -> String {
    format!(
        "GET {path} HTTP/1.1\r\nAccept-Encoding: identity\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: python-jsonschema \
         (deprecated $ref resolution)\r\nConnection: close\r\n\r\n"
    )
}

/// `UrlopenRetriever` sends `urlopen`'s request for a remote `$ref`, answers the body of a document and raises what
/// `urlopen` raises for an error status.
#[test]
fn urlopen_retriever_opens_a_document_and_raises_an_http_error_over_loopback() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = serve(listener, 2);
    let retriever = UrlopenRetriever::new(UrlopenRetrieverOptions {
        trusted_certificates: None,
    });
    let mut body = String::new();
    retriever
        .open(&format!("http://127.0.0.1:{port}/schema.json"))?
        .read_to_string(&mut body)?;
    assert_eq!(body, r#"{"type": "integer"}"#);
    let missing = retriever.open(&format!("http://127.0.0.1:{port}/missing.json")).err();
    assert_eq!(
        missing,
        Some(RemoteSchemaError::new("HTTPError", "HTTP Error 404: Not Found"))
    );
    let heads = server.join().map_err(|_| "the server thread panicked")??;
    assert_eq!(
        heads,
        [request_head(port, "/schema.json"), request_head(port, "/missing.json")]
    );
    Ok(())
}

/// The server end of each connection a holding server accepted, with the path it asked for.
type Accepted = Arc<Mutex<Vec<(String, TcpStream)>>>;

fn field<'a>(value: &'a Value, name: &str) -> Fallible<&'a Value> {
    value.get(name).ok_or_else(|| format!("missing {name}").into())
}

fn text<'a>(value: &'a Value, name: &str) -> Fallible<&'a str> {
    field(value, name)?
        .as_str()
        .ok_or_else(|| format!("{name} is not text").into())
}

fn list<'a>(value: &'a Value, name: &str) -> Fallible<&'a [Value]> {
    field(value, name)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{name} is not a list").into())
}

fn object(value: Value) -> Fallible<JsonObject> {
    Ok(serde_json::from_value(value)?)
}

/// The port of a loopback server that answers each connection with the recorded response of its path, shuts its
/// writing half when the recording did, and keeps its end open so the test can see how the client ended it. The kept
/// end gets its [`CLOSE_WAIT`] read timeout before the response is written and the writing half shut: macOS refuses to
/// set options on a socket once it is shut in both directions.
fn holding_server(routes: &[Value]) -> Fallible<(u16, Accepted)> {
    let mut responses = HashMap::new();
    for route in routes {
        let response = text(route, "response")?
            .chars()
            .map(u8::try_from)
            .collect::<Result<Vec<u8>, _>>()?;
        let shutdown = field(route, "shutdown")?.as_bool().ok_or("shutdown is not a boolean")?;
        responses.insert(text(route, "path")?.to_owned(), (response, shutdown));
    }
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let accepted: Accepted = Arc::default();
    let sockets = Arc::clone(&accepted);
    thread::spawn(move || {
        for mut socket in listener.incoming().map_while(Result::ok) {
            let mut head = Vec::new();
            let mut buffer = [0u8; 4096];
            while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                match socket.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => head.extend_from_slice(buffer.get(..read).unwrap_or_default()),
                }
            }
            let path = head
                .split(|&byte| byte == b' ')
                .nth(1)
                .map(|path| String::from_utf8_lossy(path).into_owned())
                .unwrap_or_default();
            let Some((response, shutdown)) = responses.get(&path) else {
                continue;
            };
            if let Ok(kept) = socket
                .set_read_timeout(Some(CLOSE_WAIT))
                .and_then(|()| socket.try_clone())
            {
                sockets
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((path, kept));
            }
            let written = socket.write_all(response);
            if written.is_ok() && *shutdown {
                let _closed_by_the_client = socket.shutdown(Shutdown::Write);
            }
        }
    });
    Ok((port, accepted))
}

/// How the client ended the connection, as the server end sees it within the [`CLOSE_WAIT`] the holding server set:
/// `fin`, `rst`, `data` when it sent more, or `open`.
fn client_end(socket: &TcpStream) -> &'static str {
    let mut byte = [0u8; 1];
    match socket.peek(&mut byte) {
        Ok(0) => "fin",
        Ok(_) => "data",
        Err(error) if error.kind() == ErrorKind::ConnectionReset => "rst",
        Err(_) => "open",
    }
}

/// A Python dialect server with one tool, `t`, whose handler answers `ok`, declaring `schema` and retrieving with
/// `retriever`.
fn python_server(schema: Value, retriever: &Arc<UrlopenRetriever>) -> Fallible<LocalMcpServer> {
    let tool = SdkTool::from(SdkToolOptions {
        name: McpToolName::from("t"),
        description: String::new(),
        input_schema: McpToolInputSchema::new(object(schema)?),
        handler: Arc::new(|_: McpToolRequest| async { Ok::<_, CallbackError>(McpToolOutput::text("ok")) }),
        annotations: None,
        search_hint: None,
        always_load: false,
    });
    let retriever: Arc<dyn RemoteSchemaRetriever> = Arc::<UrlopenRetriever>::clone(retriever);
    Ok(LocalMcpServer::from(LocalMcpServerOptions {
        tools: Some(tool.into()),
        dialect: McpServerDialect::Python,
        ..LocalMcpServerOptions::new(McpServerName::from("s"))
    })
    .with_remote_schema_retriever(retriever))
}

/// Initializes a session of `server`, calls `t` with `arguments` and answers the text of the result.
async fn call_text(server: &LocalMcpServer, arguments: Value) -> Fallible<String> {
    let session: Arc<dyn SdkMcpSession> = server.open_session();
    let requests = [
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
            "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}
        }}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"t","arguments":arguments}}),
    ];
    let mut last = None;
    for request in requests {
        last = session.request(object(request)?, CancellationToken::new()).await?;
    }
    let response = last.ok_or("missing tools/call response")?;
    Ok(response
        .get("result")
        .and_then(|result| result.pointer("/content/0/text"))
        .and_then(Value::as_str)
        .ok_or("tools/call without a text result")?
        .to_owned())
}

/// The Python dialect closes every connection it opens for a remote `$ref` before the call answers, as the Python
/// bridge has when it writes the tool result: at the end of the body, after reading a redirect's body, and at once on
/// an error, with an `RST` when the error left part of the body unread.
#[tokio::test]
async fn python_dialect_closes_each_retrieval_connection_before_the_call_answers() -> TestResult {
    let fixture: Value = serde_json::from_str(CLOSE_FIXTURE)?;
    let (port, accepted) = holding_server(list(&fixture, "routes")?)?;
    let retriever = Arc::new(UrlopenRetriever::new(UrlopenRetrieverOptions {
        trusted_certificates: None,
    }));
    let cases = list(&fixture, "cases")?;
    for case in cases {
        let path = text(case, "path")?;
        let uri = format!("http://127.0.0.1:{port}{path}");
        let server = python_server(
            json!({"type": "object", "properties": {"v": {"$ref": uri}}}),
            &retriever,
        )?;
        let answer = call_text(&server, json!({"v": "x"})).await?;
        let taken = std::mem::take(&mut *accepted.lock().unwrap_or_else(PoisonError::into_inner));
        let ends: Vec<(String, &str)> = taken
            .iter()
            .map(|(path, socket)| (path.clone(), client_end(socket)))
            .collect();
        let expected = text(case, "result")?.replace("{PORT}", &port.to_string());
        assert_eq!(answer, expected, "{path}");
        let recorded: Vec<(String, String)> = serde_json::from_value(field(case, "connections")?.clone())?;
        let recorded: Vec<(String, &str)> = recorded
            .iter()
            .map(|(path, end)| (path.clone(), end.as_str()))
            .collect();
        assert_eq!(ends, recorded, "{path}");
    }
    assert_eq!(cases.len(), 10);
    Ok(())
}
