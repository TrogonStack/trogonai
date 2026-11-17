//! The request for the read stream carries what the pinned upstream bundle sent (the `stream` cases of
//! `tests/fixtures/bridge_worker/goldens.json`: path, query and headers), and the answer reaches the stream loop as a
//! status and a body read a chunk at a time.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use reqwest::ResponseBuilderExt;
use serde_json::Value;
use trogon_claude_agent_sdk::adapters::HttpExecute;
use trogon_claude_agent_sdk::{
    HttpStreamConnector, NonOrigin403, SensitiveString, StreamBody, StreamConnector, StreamRequest, TokioPacing,
    WorkerClient, WorkerConfig,
};

const GOLDENS: &str = include_str!("fixtures/bridge_worker/goldens.json");

/// What the fake service answers: a request that fails, or a status with a body of chunks, the last of which may
/// break.
enum Reply {
    Fail,
    Answer {
        status: u16,
        chunks: Vec<&'static str>,
        breaks: bool,
    },
}

type Seen = Arc<Mutex<Vec<(String, String, Vec<(String, String)>)>>>;

struct Service {
    seen: Seen,
    replies: Mutex<VecDeque<Reply>>,
}

impl HttpExecute for Service {
    fn execute(&self, request: reqwest::Request) -> impl Future<Output = reqwest::Result<reqwest::Response>> + Send {
        let mut headers: Vec<(String, String)> = request
            .headers()
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_owned()))
            .collect();
        headers.sort();
        let url = request.url().clone();
        self.seen
            .lock()
            .unwrap()
            .push((request.method().to_string(), url.to_string(), headers));
        let reply = self.replies.lock().unwrap().pop_front().expect("a scripted reply");
        async move {
            match reply {
                Reply::Fail => reqwest::Client::new().get("not a url").build().map(|_| unreachable!()),
                Reply::Answer { status, chunks, breaks } => {
                    let mut items: Vec<Result<Vec<u8>, std::io::Error>> =
                        chunks.into_iter().map(|chunk| Ok(chunk.as_bytes().to_vec())).collect();
                    if breaks {
                        items.push(Err(std::io::Error::other("reset")));
                    }
                    let body = reqwest::Body::wrap_stream(futures_util::stream::iter(items));
                    let response = http::Response::builder().status(status).url(url).body(body).unwrap();
                    Ok(reqwest::Response::from(response))
                }
            }
        }
    }
}

fn connector(
    token: &str,
    replies: Vec<Reply>,
) -> (
    HttpStreamConnector<Service, TokioPacing>,
    Seen,
    Arc<WorkerClient<Service, TokioPacing>>,
) {
    let seen = Seen::default();
    let service = Service {
        seen: Arc::clone(&seen),
        replies: Mutex::new(replies.into()),
    };
    let client = Arc::new(
        WorkerClient::new(
            service,
            TokioPacing,
            WorkerConfig {
                api_base_url: "http://service.invalid".to_owned(),
                session_id: "cse_abc".to_owned(),
                token: SensitiveString::from(token),
                entrypoint: None,
                heartbeat_interval_ms: None,
                session_access_token: None,
                non_origin_403: NonOrigin403::default(),
            },
        )
        .expect("a worker"),
    );
    (HttpStreamConnector::new(Arc::clone(&client)), seen, client)
}

fn ok() -> Reply {
    Reply::Answer {
        status: 200,
        chunks: vec![],
        breaks: false,
    }
}

fn lowered(headers: &Value) -> Vec<(String, String)> {
    let mut list: Vec<(String, String)> = headers
        .as_array()
        .unwrap()
        .iter()
        .map(|pair| {
            (
                pair[0].as_str().unwrap().to_lowercase(),
                pair[1].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    list.sort();
    list
}

#[tokio::test]
async fn every_recorded_stream_request_is_reproduced() {
    let goldens: Value = serde_json::from_str(GOLDENS).unwrap();
    let mut checked = 0;
    for case in goldens["stream"].as_array().unwrap() {
        for recorded in case["requests"].as_array().unwrap() {
            let path = recorded["path"].as_str().unwrap();
            if !path.starts_with("/worker/events/stream") {
                continue;
            }
            let headers = lowered(&recorded["headers"]);
            let wanted = StreamRequest {
                query: path.split_once('?').map(|(_, query)| query.to_owned()),
                last_event_id: headers
                    .iter()
                    .find(|(name, _)| name == "last-event-id")
                    .map(|(_, value)| value.clone()),
            };
            let (connector, seen, _) = connector("tok", vec![ok()]);
            let answer = connector.open(&wanted).await.map_err(|_| ()).expect("an answer");
            assert_eq!(answer.status, 200);
            let seen = seen.lock().unwrap();
            let (method, url, sent) = &seen[0];
            assert_eq!(method, "GET");
            assert_eq!(url, &format!("http://service.invalid/v1/code/sessions/cse_abc{path}"));
            assert_eq!(sent, &headers);
            checked += 1;
        }
    }
    assert!(checked >= 8);
}

#[tokio::test]
async fn a_replaced_token_is_the_one_sent() {
    let (connector, seen, client) = connector("tok", vec![ok()]);
    client.set_token(SensitiveString::from("tok2"));
    let _ = connector
        .open(&StreamRequest {
            query: None,
            last_event_id: None,
        })
        .await;
    let seen = seen.lock().unwrap();
    assert!(
        seen[0]
            .2
            .contains(&("authorization".to_owned(), "Bearer tok2".to_owned()))
    );
}

#[tokio::test]
async fn a_token_that_is_not_a_header_value_gives_no_request() {
    let (connector, seen, _) = connector("t\u{3b1}", vec![]);
    let failure = connector
        .open(&StreamRequest {
            query: None,
            last_event_id: None,
        })
        .await;
    assert!(failure.is_err());
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_id_that_is_not_a_header_value_is_left_out() {
    let (connector, seen, _) = connector("tok", vec![ok()]);
    let wanted = StreamRequest {
        query: None,
        last_event_id: Some("a\nb".to_owned()),
    };
    let _ = connector.open(&wanted).await;
    assert!(
        seen.lock().unwrap()[0]
            .2
            .iter()
            .all(|(name, _)| name != "last-event-id")
    );
}

#[tokio::test]
async fn a_failed_request_is_a_failure() {
    let (connector, _, _) = connector("tok", vec![Reply::Fail]);
    let failure = connector
        .open(&StreamRequest {
            query: None,
            last_event_id: None,
        })
        .await;
    assert!(failure.is_err());
}

#[tokio::test]
async fn the_status_and_the_body_chunks_are_handed_over_and_a_broken_body_is_an_error() {
    let replies = vec![Reply::Answer {
        status: 403,
        chunks: vec!["id: 1\n", "\n"],
        breaks: true,
    }];
    let (connector, _, _) = connector("tok", replies);
    let answer = connector
        .open(&StreamRequest {
            query: None,
            last_event_id: None,
        })
        .await
        .map_err(|_| ())
        .expect("an answer");
    assert_eq!(answer.status, 403);
    let mut body = answer.body;
    assert_eq!(body.next_chunk().await.unwrap().unwrap(), b"id: 1\n");
    assert_eq!(body.next_chunk().await.unwrap().unwrap(), b"\n");
    assert!(body.next_chunk().await.unwrap().is_err());
}
