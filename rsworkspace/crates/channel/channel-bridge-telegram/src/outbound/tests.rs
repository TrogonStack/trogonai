use super::*;
use axum::extract::State;
use axum::http::Uri;
use axum::{Json, Router};
use serde_json::Value;
use tokio::sync::mpsc;

async fn reject_request(
    State(requests): State<mpsc::Sender<(String, Value)>>,
    uri: Uri,
    Json(body): Json<Value>,
) -> Json<Value> {
    requests.send((uri.path().to_owned(), body)).await.unwrap();
    Json(serde_json::from_str(r#"{"ok":false,"error_code":400,"description":"Bad Request: chat not found"}"#).unwrap())
}

#[tokio::test]
async fn outbound_adapters_forward_chat_and_text_and_preserve_api_errors() {
    let (sent, mut requests) = mpsc::channel(2);
    let app = Router::new().fallback(reject_request).with_state(sent);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                stopped.await.unwrap();
            })
            .await
            .unwrap();
    });
    let bot = Bot::new("fixture-token").set_api_url(format!("http://{address}").parse().unwrap());
    let outbound = TelegramOutbound::new(bot);
    assert!(matches!(outbound.typing(42).await, Err(teloxide::RequestError::Api(_))));
    assert!(matches!(
        outbound.send_text(-7, "hello".into()).await,
        Err(teloxide::RequestError::Api(_))
    ));
    let (typing_path, typing) = requests.recv().await.unwrap();
    assert!(typing_path.ends_with("/SendChatAction"));
    assert_eq!(typing["chat_id"], 42);
    assert_eq!(typing["action"], "typing");
    let (text_path, text) = requests.recv().await.unwrap();
    assert!(text_path.ends_with("/SendMessage"));
    assert_eq!(text["chat_id"], -7);
    assert_eq!(text["text"], "hello");
    stop.send(()).unwrap();
    server.await.unwrap();
}
