use async_nats::jetstream::consumer::{Consumer, pull};
use futures::StreamExt;
use reqwest::Client as HttpClient;
use slack_morphism::prelude::*;
use slack_types::events::{
    SlackOutboundMessage, SlackReactionAction, SlackStreamAppendMessage, SlackStreamStopMessage,
    SlackViewOpenRequest, SlackViewPublishRequest,
};
use std::sync::Arc;
use std::time::Duration;

use crate::format;

/// Slack rate limit for `chat.postMessage`: ~1 request/second per channel.
/// We use a conservative global delay to stay safely below the limit.
const POST_MESSAGE_DELAY: Duration = Duration::from_millis(1_100);

const SLACK_API_BASE: &str = "https://slack.com";

pub async fn run_outbound_loop(
    consumer: Consumer<pull::Config>,
    slack_client: Arc<SlackHyperClient>,
    bot_token: String,
    http_client: Arc<HttpClient>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create outbound message stream");
            return;
        }
    };
    let raw_token = bot_token.clone();
    let token = SlackApiToken::new(bot_token.into());

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on outbound consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackOutboundMessage>(&msg.payload) {
            Ok(outbound) => {
                let session = slack_client.open_session(&token);
                let channel: SlackChannelId = outbound.channel.into();
                let thread_ts: Option<SlackTs> = outbound.thread_ts.map(|ts| ts.into());

                let converted_text = format::markdown_to_mrkdwn(&outbound.text);

                let blocks: Option<Vec<SlackBlock>> = outbound.blocks.as_ref().and_then(|v| {
                    let bytes = serde_json::to_vec(v).ok()?;
                    serde_json::from_slice::<Vec<SlackBlock>>(&bytes)
                        .map_err(|e| {
                            tracing::warn!(error = %e, "Failed to parse blocks JSON, falling back to text");
                        })
                        .ok()
                });

                if let Some(ref media_url) = outbound.media_url {
                    match upload_file_to_slack(
                        &http_client,
                        &raw_token,
                        &channel,
                        thread_ts.as_ref(),
                        &converted_text,
                        media_url,
                        SLACK_API_BASE,
                    )
                    .await
                    {
                        Ok(()) => {
                            let _ = msg.ack().await;
                            continue;
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                url = %media_url,
                                "Media upload failed, falling back to text-only post"
                            );
                        }
                    }
                }

                let chunks = format::chunk_text(&converted_text, format::SLACK_TEXT_LIMIT);
                if chunks.is_empty() {
                    let _ = msg.ack().await;
                    continue;
                }

                let first_text = chunks[0].clone();
                let mut content = SlackMessageContent::new().with_text(first_text);
                if chunks.len() == 1
                    && let Some(ref blks) = blocks
                {
                    content = content.with_blocks(blks.clone());
                }

                let mut request = SlackApiChatPostMessageRequest::new(channel.clone(), content);
                if let Some(ref ts) = thread_ts {
                    request = request.with_thread_ts(ts.clone());
                }
                if let Some(ref username) = outbound.username {
                    request = request.with_username(username.clone());
                }
                if let Some(ref icon_url) = outbound.icon_url {
                    request = request.with_icon_url(icon_url.clone());
                }

                let post_result = {
                    let r = session.chat_post_message(&request).await;
                    if let Err(ref e) = r
                        && format!("{e}").contains("ratelimited")
                    {
                        tracing::warn!("Slack rate limit on postMessage, retrying after 5 s");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        slack_client
                            .open_session(&token)
                            .chat_post_message(&request)
                            .await
                    } else {
                        r
                    }
                };
                let first_ts = match post_result {
                    Ok(resp) => {
                        tokio::time::sleep(POST_MESSAGE_DELAY).await;
                        Some(resp.ts)
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to send message to Slack");
                        None
                    }
                };

                if chunks.len() > 1 {
                    let reply_ts = first_ts.or(thread_ts.clone());
                    for (idx, chunk) in chunks[1..].iter().enumerate() {
                        let is_last = idx == chunks.len() - 2;
                        let mut chunk_content = SlackMessageContent::new().with_text(chunk.clone());
                        if is_last && let Some(ref blks) = blocks {
                            chunk_content = chunk_content.with_blocks(blks.clone());
                        }
                        let mut chunk_req =
                            SlackApiChatPostMessageRequest::new(channel.clone(), chunk_content);
                        if let Some(ref ts) = reply_ts {
                            chunk_req = chunk_req.with_thread_ts(ts.clone());
                        }
                        if let Some(ref username) = outbound.username {
                            chunk_req = chunk_req.with_username(username.clone());
                        }
                        if let Some(ref icon_url) = outbound.icon_url {
                            chunk_req = chunk_req.with_icon_url(icon_url.clone());
                        }
                        if let Err(e) = session.chat_post_message(&chunk_req).await {
                            tracing::error!(error = %e, "Failed to send message chunk to Slack");
                        }
                        tokio::time::sleep(POST_MESSAGE_DELAY).await;
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize outbound NATS message");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK JetStream outbound message");
        }
    }
}

pub async fn run_stream_append_loop(
    consumer: Consumer<pull::Config>,
    slack_client: Arc<SlackHyperClient>,
    bot_token: String,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create stream_append message stream");
            return;
        }
    };
    let token = SlackApiToken::new(bot_token.into());

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on stream_append consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackStreamAppendMessage>(&msg.payload) {
            Ok(append) => {
                let session = slack_client.open_session(&token);
                let channel: SlackChannelId = append.channel.into();
                let ts: SlackTs = append.ts.into();
                let converted = format::markdown_to_mrkdwn(&append.text);
                let content = SlackMessageContent::new().with_text(converted);
                let request = SlackApiChatUpdateRequest::new(channel, content, ts);
                let update_result = {
                    let r = session.chat_update(&request).await;
                    if let Err(ref e) = r
                        && format!("{e}").contains("ratelimited")
                    {
                        tracing::warn!(
                            "Slack rate limit on chat.update (append), retrying after 5 s"
                        );
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        slack_client
                            .open_session(&token)
                            .chat_update(&request)
                            .await
                    } else {
                        r
                    }
                };
                if let Err(e) = update_result {
                    tracing::error!(error = %e, "Failed to update streaming message (append)");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize stream append NATS message");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK JetStream stream_append message");
        }
    }
}

pub async fn run_stream_stop_loop(
    consumer: Consumer<pull::Config>,
    slack_client: Arc<SlackHyperClient>,
    bot_token: String,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create stream_stop message stream");
            return;
        }
    };
    let token = SlackApiToken::new(bot_token.into());

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on stream_stop consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackStreamStopMessage>(&msg.payload) {
            Ok(stop) => {
                let session = slack_client.open_session(&token);
                let channel: SlackChannelId = stop.channel.into();
                let ts: SlackTs = stop.ts.into();

                let blocks: Option<Vec<SlackBlock>> = stop.blocks.as_ref().and_then(|v| {
                    let bytes = serde_json::to_vec(v).ok()?;
                    serde_json::from_slice::<Vec<SlackBlock>>(&bytes)
                        .map_err(|e| {
                            tracing::warn!(error = %e, "Failed to parse stop blocks JSON");
                        })
                        .ok()
                });

                let converted_final = format::markdown_to_mrkdwn(&stop.final_text);
                let mut content = SlackMessageContent::new().with_text(converted_final);
                if let Some(blks) = blocks {
                    content = content.with_blocks(blks);
                }

                let request = SlackApiChatUpdateRequest::new(channel, content, ts);
                let update_result = {
                    let r = session.chat_update(&request).await;
                    if let Err(ref e) = r
                        && format!("{e}").contains("ratelimited")
                    {
                        tracing::warn!(
                            "Slack rate limit on chat.update (stop), retrying after 5 s"
                        );
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        slack_client
                            .open_session(&token)
                            .chat_update(&request)
                            .await
                    } else {
                        r
                    }
                };
                if let Err(e) = update_result {
                    tracing::error!(error = %e, "Failed to update streaming message (stop)");
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize stream stop NATS message");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK JetStream stream_stop message");
        }
    }
}

pub async fn run_reaction_action_loop(
    consumer: Consumer<pull::Config>,
    slack_client: Arc<SlackHyperClient>,
    bot_token: String,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create reaction_action message stream");
            return;
        }
    };
    let token = SlackApiToken::new(bot_token.into());

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on reaction_action consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackReactionAction>(&msg.payload) {
            Ok(action) => {
                let session = slack_client.open_session(&token);
                let channel = SlackChannelId(action.channel.clone());
                let name = SlackReactionName(action.reaction.clone());
                let timestamp = SlackTs(action.ts.clone());

                let outcome = if action.add {
                    session
                        .reactions_add(&SlackApiReactionsAddRequest {
                            channel,
                            name,
                            timestamp,
                        })
                        .await
                        .map(|_| ())
                } else {
                    session
                        .reactions_remove(&SlackApiReactionsRemoveRequest {
                            channel: Some(channel),
                            name,
                            timestamp: Some(timestamp),
                            file: None,
                            full: None,
                        })
                        .await
                        .map(|_| ())
                };

                if let Err(e) = outcome {
                    tracing::warn!(
                        error = %e,
                        reaction = %action.reaction,
                        add = action.add,
                        "Failed to update reaction on Slack message"
                    );
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize SlackReactionAction");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK reaction_action message");
        }
    }
}

// ── File upload ───────────────────────────────────────────────────────────────

/// Derive a filename from the last path segment of a URL, stripping query params.
fn extract_upload_filename(url: &str) -> String {
    url.rsplit('/')
        .next()
        .and_then(|s| s.split('?').next())
        .filter(|s| !s.is_empty())
        .unwrap_or("file")
        .to_string()
}

/// Upload a file to Slack using the two-step API introduced in 2023:
/// 1. `files.getUploadURLExternal` — obtain a pre-signed upload URL + file ID
/// 2. PUT the raw bytes to that URL
/// 3. `files.completeUploadExternal` — associate the file with a channel
///
/// `api_base` should be `"https://slack.com"` in production; it is exposed for
/// testing against a local mock server.
///
/// On success the file appears as a native Slack attachment; the caller should
/// skip the plain-text fallback post.  On failure the error is returned so the
/// caller can fall back to text-only.
async fn upload_file_to_slack(
    http: &HttpClient,
    token: &str,
    channel: &SlackChannelId,
    thread_ts: Option<&SlackTs>,
    text: &str,
    media_url: &str,
    api_base: &str,
) -> Result<(), String> {
    // 1. Download the file from the media URL.
    let download = http
        .get(media_url)
        .send()
        .await
        .map_err(|e| format!("download request: {e}"))?;

    let filename = extract_upload_filename(media_url);

    let content_type = download
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let bytes = download
        .bytes()
        .await
        .map_err(|e| format!("download body: {e}"))?;
    let length = bytes.len();
    let length_str = length.to_string();

    // 2. Ask Slack for a pre-signed upload URL (retry once on rate limit).
    let get_url_url = format!("{api_base}/api/files.getUploadURLExternal");
    let make_get_url_req = || {
        http.post(&get_url_url)
            .bearer_auth(token)
            .form(&[("filename", filename.as_str()), ("length", length_str.as_str())])
    };
    let get_url: serde_json::Value = {
        let r: serde_json::Value = make_get_url_req()
            .send()
            .await
            .map_err(|e| format!("getUploadURLExternal request: {e}"))?
            .json()
            .await
            .map_err(|e| format!("getUploadURLExternal parse: {e}"))?;
        if !r["ok"].as_bool().unwrap_or(false) && r["error"].as_str() == Some("ratelimited") {
            tracing::warn!("Slack rate limit on getUploadURLExternal, retrying after 5 s");
            tokio::time::sleep(Duration::from_secs(5)).await;
            make_get_url_req()
                .send()
                .await
                .map_err(|e| format!("getUploadURLExternal retry request: {e}"))?
                .json()
                .await
                .map_err(|e| format!("getUploadURLExternal retry parse: {e}"))?
        } else {
            r
        }
    };

    if !get_url["ok"].as_bool().unwrap_or(false) {
        return Err(format!(
            "files.getUploadURLExternal: {}",
            get_url["error"].as_str().unwrap_or("unknown")
        ));
    }

    let upload_url = get_url["upload_url"]
        .as_str()
        .ok_or_else(|| "missing upload_url in Slack response".to_string())?
        .to_string();
    let file_id = get_url["file_id"]
        .as_str()
        .ok_or_else(|| "missing file_id in Slack response".to_string())?
        .to_string();

    // 3. PUT the raw bytes to the pre-signed URL.
    http.put(&upload_url)
        .header(reqwest::header::CONTENT_TYPE, &content_type)
        .body(bytes)
        .send()
        .await
        .map_err(|e| format!("upload PUT: {e}"))?;

    // 4. Complete the upload and associate it with the channel (retry once on rate limit).
    let mut body = serde_json::Map::new();
    body.insert("files".into(), serde_json::json!([{"id": file_id}]));
    body.insert(
        "channel_id".into(),
        serde_json::Value::String(channel.0.clone()),
    );
    if !text.is_empty() {
        body.insert(
            "initial_comment".into(),
            serde_json::Value::String(text.to_string()),
        );
    }
    if let Some(ts) = thread_ts {
        body.insert(
            "thread_ts".into(),
            serde_json::Value::String(ts.0.clone()),
        );
    }

    let complete_url = format!("{api_base}/api/files.completeUploadExternal");
    let complete: serde_json::Value = {
        let r: serde_json::Value = http
            .post(&complete_url)
            .bearer_auth(token)
            .json(&serde_json::Value::Object(body.clone()))
            .send()
            .await
            .map_err(|e| format!("completeUploadExternal request: {e}"))?
            .json()
            .await
            .map_err(|e| format!("completeUploadExternal parse: {e}"))?;
        if !r["ok"].as_bool().unwrap_or(false) && r["error"].as_str() == Some("ratelimited") {
            tracing::warn!("Slack rate limit on completeUploadExternal, retrying after 5 s");
            tokio::time::sleep(Duration::from_secs(5)).await;
            http.post(&complete_url)
                .bearer_auth(token)
                .json(&serde_json::Value::Object(body))
                .send()
                .await
                .map_err(|e| format!("completeUploadExternal retry request: {e}"))?
                .json()
                .await
                .map_err(|e| format!("completeUploadExternal retry parse: {e}"))?
        } else {
            r
        }
    };

    if !complete["ok"].as_bool().unwrap_or(false) {
        return Err(format!(
            "files.completeUploadExternal: {}",
            complete["error"].as_str().unwrap_or("unknown")
        ));
    }

    Ok(())
}

// ── View modal loops ──────────────────────────────────────────────────────────

pub async fn run_view_open_loop(
    consumer: Consumer<pull::Config>,
    bot_token: String,
    http_client: Arc<HttpClient>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create view_open message stream");
            return;
        }
    };
    let url = format!("{SLACK_API_BASE}/api/views.open");

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on view_open consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackViewOpenRequest>(&msg.payload) {
            Ok(req) => {
                match http_client
                    .post(&url)
                    .bearer_auth(&bot_token)
                    .json(&req)
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(body) if !body["ok"].as_bool().unwrap_or(false) => {
                            tracing::error!(
                                api_error = body["error"].as_str().unwrap_or("unknown"),
                                "views.open failed"
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to parse views.open response");
                        }
                    },
                    Err(e) => tracing::error!(error = %e, "HTTP error calling views.open"),
                }
            }
            Err(e) => tracing::error!(error = %e, "Failed to deserialize SlackViewOpenRequest"),
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK view_open message");
        }
    }
}

pub async fn run_view_publish_loop(
    consumer: Consumer<pull::Config>,
    bot_token: String,
    http_client: Arc<HttpClient>,
) {
    let mut messages = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create view_publish message stream");
            return;
        }
    };
    let url = format!("{SLACK_API_BASE}/api/views.publish");

    while let Some(result) = messages.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(error = %e, "JetStream error on view_publish consumer");
                continue;
            }
        };

        match serde_json::from_slice::<SlackViewPublishRequest>(&msg.payload) {
            Ok(req) => {
                match http_client
                    .post(&url)
                    .bearer_auth(&bot_token)
                    .json(&req)
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(body) if !body["ok"].as_bool().unwrap_or(false) => {
                            tracing::error!(
                                api_error = body["error"].as_str().unwrap_or("unknown"),
                                "views.publish failed"
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to parse views.publish response");
                        }
                    },
                    Err(e) => tracing::error!(error = %e, "HTTP error calling views.publish"),
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to deserialize SlackViewPublishRequest");
            }
        }

        if let Err(e) = msg.ack().await {
            tracing::error!(error = %e, "Failed to ACK view_publish message");
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn channel(id: &str) -> SlackChannelId {
        SlackChannelId(id.to_string())
    }

    // ── extract_upload_filename ───────────────────────────────────────────────

    #[test]
    fn filename_basic_path() {
        assert_eq!(extract_upload_filename("https://example.com/foo/bar.png"), "bar.png");
    }

    #[test]
    fn filename_strips_query() {
        assert_eq!(
            extract_upload_filename("https://cdn.example.com/image.jpg?X-Amz-Expires=300"),
            "image.jpg"
        );
    }

    #[test]
    fn filename_trailing_slash_falls_back() {
        assert_eq!(extract_upload_filename("https://example.com/path/"), "file");
    }

    #[test]
    fn filename_no_path_falls_back() {
        assert_eq!(extract_upload_filename("https://example.com"), "example.com");
    }

    // ── upload_file_to_slack (wiremock) ───────────────────────────────────────

    #[tokio::test]
    async fn upload_success() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/media/photo.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"PNGDATA".as_ref())
                    .insert_header("content-type", "image/png"),
            )
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "upload_url": format!("{}/upload-slot", server.uri()),
                "file_id": "FTEST"
            })))
            .mount(&server)
            .await;

        Mock::given(method("PUT"))
            .and(path("/upload-slot"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/files.completeUploadExternal"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})),
            )
            .mount(&server)
            .await;

        let result = upload_file_to_slack(
            &HttpClient::new(),
            "xoxb-test",
            &channel("C123"),
            None,
            "look at this",
            &format!("{}/media/photo.png", server.uri()),
            &server.uri(),
        )
        .await;

        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn upload_with_thread_ts_and_empty_text() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/f.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello".as_ref()))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "upload_url": format!("{}/slot", server.uri()),
                "file_id": "F2"
            })))
            .mount(&server)
            .await;

        Mock::given(method("PUT"))
            .and(path("/slot"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/files.completeUploadExternal"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})),
            )
            .mount(&server)
            .await;

        let ts = SlackTs("1234567890.000100".to_string());
        let result = upload_file_to_slack(
            &HttpClient::new(),
            "xoxb-test",
            &channel("C456"),
            Some(&ts),
            "",
            &format!("{}/f.txt", server.uri()),
            &server.uri(),
        )
        .await;

        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn upload_geturl_error_propagated() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/file.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"data".as_ref()))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"ok": false, "error": "not_allowed"}),
            ))
            .mount(&server)
            .await;

        let result = upload_file_to_slack(
            &HttpClient::new(),
            "xoxb-test",
            &channel("C123"),
            None,
            "",
            &format!("{}/file.bin", server.uri()),
            &server.uri(),
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not_allowed"), "expected not_allowed in error");
    }

    #[tokio::test]
    async fn upload_complete_error_propagated() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/f.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"x".as_ref()))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/files.getUploadURLExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "upload_url": format!("{}/slot", server.uri()),
                "file_id": "F3"
            })))
            .mount(&server)
            .await;

        Mock::given(method("PUT"))
            .and(path("/slot"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/files.completeUploadExternal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"ok": false, "error": "invalid_auth"}),
            ))
            .mount(&server)
            .await;

        let result = upload_file_to_slack(
            &HttpClient::new(),
            "xoxb-test",
            &channel("C123"),
            None,
            "",
            &format!("{}/f.bin", server.uri()),
            &server.uri(),
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid_auth"), "expected invalid_auth in error");
    }
}
