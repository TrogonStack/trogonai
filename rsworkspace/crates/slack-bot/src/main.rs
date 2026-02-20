mod config;
mod format;
mod health;
mod listener;
mod rate_limit;
mod sender;
mod webhook;

use async_nats::jetstream;
use config::SlackBotConfig;
use futures::StreamExt;
use listener::{
    BotState, error_handler, handle_command_event, handle_interaction_event, handle_push_event,
};
use sender::{
    run_delete_file_loop, run_delete_loop, run_ephemeral_loop, run_outbound_loop,
    run_proactive_loop, run_reaction_action_loop, run_set_status_loop,
    run_stream_append_loop, run_stream_stop_loop, run_suggested_prompts_loop, run_update_loop,
    run_upload_loop, run_view_open_loop, run_view_publish_loop,
};
use slack_morphism::prelude::*;
use slack_nats::setup::ensure_slack_stream;
use slack_nats::subscriber::{
    create_delete_consumer, create_delete_file_consumer, create_ephemeral_consumer,
    create_outbound_consumer, create_proactive_consumer, create_reaction_action_consumer,
    create_set_status_consumer, create_stream_append_consumer, create_stream_stop_consumer,
    create_suggested_prompts_consumer, create_update_consumer, create_upload_consumer,
    create_view_open_consumer, create_view_publish_consumer,
};
use rate_limit::RateLimiter;
use slack_types::events::{
    SlackListConversationsChannel, SlackListConversationsRequest, SlackListConversationsResponse,
    SlackListUsersRequest, SlackListUsersResponse, SlackListUsersUser, SlackReadMessage,
    SlackReadMessagesRequest, SlackReadMessagesResponse,
    SlackReadRepliesRequest, SlackReadRepliesResponse, SlackStreamStartRequest,
    SlackStreamStartResponse,
};
use slack_types::subjects::{
    SLACK_OUTBOUND_LIST_CONVERSATIONS, SLACK_OUTBOUND_LIST_USERS, SLACK_OUTBOUND_READ_MESSAGES,
    SLACK_OUTBOUND_READ_REPLIES, SLACK_OUTBOUND_STREAM_START,
};
use std::sync::Arc;
use std::time::Duration;
use trogon_nats::connect;
use trogon_std::env::SystemEnv;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let config = SlackBotConfig::from_env(&SystemEnv);
    let rate_limiter = Arc::new(RateLimiter::new(config.slack_api_rps));

    tracing::info!(port = config.health_port, "Starting health check server...");
    tokio::spawn(health::start_health_server(config.health_port));

    let nats_client = connect(&config.nats)
        .await
        .map_err(|e| format!("{:?}", e))?;
    let nats_client = Arc::new(nats_client);

    tracing::info!("Setting up JetStream stream...");
    let js = Arc::new(jetstream::new((*nats_client).clone()));
    ensure_slack_stream(&js).await?;

    tracing::info!(port = config.events_port, "Starting Slack Events webhook server...");
    tokio::spawn(webhook::start_webhook_server(
        config.events_port,
        config.signing_secret.clone(),
        js.clone(),
    ));

    tracing::info!("Creating JetStream consumers...");
    let outbound_consumer = create_outbound_consumer(&js).await?;
    let stream_append_consumer = create_stream_append_consumer(&js).await?;
    let stream_stop_consumer = create_stream_stop_consumer(&js).await?;
    let reaction_action_consumer = create_reaction_action_consumer(&js).await?;
    let view_open_consumer = create_view_open_consumer(&js).await?;
    let view_publish_consumer = create_view_publish_consumer(&js).await?;
    let set_status_consumer = create_set_status_consumer(&js).await?;
    let delete_consumer = create_delete_consumer(&js).await?;
    let update_consumer = create_update_consumer(&js).await?;
    let upload_consumer = create_upload_consumer(&js).await?;
    let suggested_prompts_consumer = create_suggested_prompts_consumer(&js).await?;
    let proactive_consumer = create_proactive_consumer(&js).await?;
    let ephemeral_consumer = create_ephemeral_consumer(&js).await?;
    let delete_file_consumer = create_delete_file_consumer(&js).await?;

    // stream.start uses Core NATS request/reply — subscribe on the raw client.
    let mut stream_start_sub = nats_client.subscribe(SLACK_OUTBOUND_STREAM_START).await?;
    let mut read_messages_sub = nats_client.subscribe(SLACK_OUTBOUND_READ_MESSAGES).await?;
    let mut read_replies_sub = nats_client.subscribe(SLACK_OUTBOUND_READ_REPLIES).await?;
    let mut list_users_sub = nats_client.subscribe(SLACK_OUTBOUND_LIST_USERS).await?;
    let mut list_conversations_sub = nats_client.subscribe(SLACK_OUTBOUND_LIST_CONVERSATIONS).await?;

    tracing::info!("Connecting to Slack via Socket Mode...");
    let slack_client = Arc::new(SlackClient::new(SlackClientHyperConnector::new()?));

    let bot_state = BotState {
        nats: js.clone(),
        bot_user_id: config.bot_user_id.clone(),
        mention_gating: config.mention_gating,
        mention_gating_channels: config.mention_gating_channels.clone(),
        no_mention_channels: config.no_mention_channels.clone(),
        mention_patterns: config.mention_patterns.clone(),
        allow_bots: config.allow_bots,
        bot_token: config.bot_token.clone(),
        http_client: reqwest::Client::new(),
        media_max_mb: config.media_max_mb,
        user_token: config.user_token.clone(),
    };

    let socket_mode_callbacks = SlackSocketModeListenerCallbacks::new()
        .with_push_events(handle_push_event)
        .with_command_events(handle_command_event)
        .with_interaction_events(handle_interaction_event);

    let listener_environment = Arc::new(
        SlackClientEventsListenerEnvironment::new(slack_client.clone())
            .with_error_handler(error_handler)
            .with_user_state(bot_state),
    );

    let socket_mode_listener = SlackClientSocketModeListener::new(
        &SlackClientSocketModeConfig::new(),
        listener_environment,
        socket_mode_callbacks,
    );

    let app_token: SlackApiToken = SlackApiToken::new(config.app_token.clone().into());
    socket_mode_listener.listen_for(&app_token).await?;

    tracing::info!("Slack bot running. Press Ctrl+C to stop.");

    let bot_token = config.bot_token.clone();

    // Call auth.test to get our own bot user ID and announce it on NATS.
    {
        let auth_url = "https://slack.com/api/auth.test";
        let http_client = reqwest::Client::new();
        match http_client
            .post(auth_url)
            .header("Authorization", format!("Bearer {}", bot_token))
            .send()
            .await
        {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(v) if v["ok"].as_bool() == Some(true) => {
                    if let Some(uid) = v["user_id"].as_str() {
                        let payload = serde_json::json!({"bot_user_id": uid}).to_string();
                        let _ = nats_client
                            .publish("slack.bot.identity", payload.into())
                            .await;
                        tracing::info!(bot_user_id = %uid, "Published bot identity from auth.test");
                    }
                }
                Ok(v) => tracing::warn!(response = ?v, "auth.test returned ok=false"),
                Err(e) => tracing::warn!(error = %e, "Failed to parse auth.test response"),
            },
            Err(e) => tracing::warn!(error = %e, "auth.test request failed"),
        }
    }

    let outbound_handle = tokio::spawn({
        let sc = slack_client.clone();
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move { run_outbound_loop(outbound_consumer, sc, bt, hc, rl, config.text_chunk_limit, config.chunk_mode_newline).await }
    });
    let outbound_abort = outbound_handle.abort_handle();

    let stream_append_handle = tokio::spawn({
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move { run_stream_append_loop(stream_append_consumer, bt, hc, rl).await }
    });
    let stream_append_abort = stream_append_handle.abort_handle();

    let stream_stop_handle = tokio::spawn({
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move { run_stream_stop_loop(stream_stop_consumer, bt, hc, rl).await }
    });
    let stream_stop_abort = stream_stop_handle.abort_handle();

    let reaction_action_handle = tokio::spawn({
        let sc = slack_client.clone();
        let bt = bot_token.clone();
        let rl = rate_limiter.clone();
        async move { run_reaction_action_loop(reaction_action_consumer, sc, bt, rl).await }
    });
    let reaction_action_abort = reaction_action_handle.abort_handle();

    let view_open_handle = tokio::spawn({
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move { run_view_open_loop(view_open_consumer, bt, hc, rl).await }
    });
    let view_open_abort = view_open_handle.abort_handle();

    let view_publish_handle = tokio::spawn({
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move { run_view_publish_loop(view_publish_consumer, bt, hc, rl).await }
    });
    let view_publish_abort = view_publish_handle.abort_handle();

    let set_status_handle = tokio::spawn({
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move { run_set_status_loop(set_status_consumer, bt, hc, rl).await }
    });
    let set_status_abort = set_status_handle.abort_handle();

    let delete_handle = tokio::spawn({
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move { run_delete_loop(delete_consumer, bt, hc, rl).await }
    });
    let delete_abort = delete_handle.abort_handle();

    let update_handle = tokio::spawn({
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move { run_update_loop(update_consumer, bt, hc, rl).await }
    });
    let update_abort = update_handle.abort_handle();

    let upload_handle = tokio::spawn({
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        let nc = (*nats_client).clone();
        async move { run_upload_loop(upload_consumer, bt, hc, rl, nc).await }
    });
    let upload_abort = upload_handle.abort_handle();

    // stream.start handler: Core NATS request/reply — stays on raw client.
    let stream_start_handle = tokio::spawn({
        let nc = nats_client.clone();
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move {
            while let Some(msg) = stream_start_sub.next().await {
                let reply_to = match msg.reply {
                    Some(ref r) => r.clone(),
                    None => {
                        tracing::warn!("stream_start message has no reply subject, ignoring");
                        continue;
                    }
                };

                match serde_json::from_slice::<SlackStreamStartRequest>(&msg.payload) {
                    Ok(req) => {
                        let mut body = serde_json::Map::new();
                        body.insert("channel".into(), serde_json::Value::String(req.channel.clone()));
                        if let Some(ref tts) = req.thread_ts {
                            body.insert("thread_ts".into(), serde_json::Value::String(tts.clone()));
                        }

                        rl.acquire().await;
                        match hc
                            .post("https://slack.com/api/chat.startStream")
                            .bearer_auth(&bt)
                            .json(&serde_json::Value::Object(body))
                            .send()
                            .await
                        {
                            Ok(resp) => match resp.json::<serde_json::Value>().await {
                                Ok(api_resp) if api_resp["ok"].as_bool().unwrap_or(false) => {
                                    let channel = api_resp["channel"]
                                        .as_str()
                                        .unwrap_or(&req.channel)
                                        .to_string();
                                    let ts = api_resp["message_ts"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    let response = SlackStreamStartResponse { channel, ts };
                                    match serde_json::to_vec(&response) {
                                        Ok(bytes) => {
                                            if let Err(e) = nc.publish(reply_to, bytes.into()).await {
                                                tracing::error!(
                                                    error = %e,
                                                    "Failed to publish stream_start reply"
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!(
                                                error = %e,
                                                "Failed to serialize stream_start response"
                                            );
                                        }
                                    }
                                }
                                Ok(api_resp) => {
                                    tracing::error!(
                                        api_error = api_resp["error"].as_str().unwrap_or("unknown"),
                                        "chat.startStream failed"
                                    );
                                }
                                Err(e) => {
                                    tracing::error!(
                                        error = %e,
                                        "Failed to parse chat.startStream response"
                                    );
                                }
                            },
                            Err(e) => {
                                tracing::error!(
                                    error = %e,
                                    "HTTP error calling chat.startStream"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "Failed to deserialize stream_start NATS message"
                        );
                    }
                }
            }
        }
    });
    let stream_start_abort = stream_start_handle.abort_handle();

    // read_messages handler: Core NATS request/reply — stays on raw client.
    let read_messages_handle = tokio::spawn({
        let nc = nats_client.clone();
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move {
            while let Some(msg) = read_messages_sub.next().await {
                let reply_to = match msg.reply {
                    Some(ref r) => r.clone(),
                    None => {
                        tracing::warn!("read_messages message has no reply subject, ignoring");
                        continue;
                    }
                };

                match serde_json::from_slice::<SlackReadMessagesRequest>(&msg.payload) {
                    Ok(req) => {
                        let limit = req.limit.unwrap_or(20).min(200);
                        let mut query: Vec<(&str, String)> = vec![
                            ("channel", req.channel.clone()),
                            ("limit", limit.to_string()),
                        ];
                        if let Some(ref oldest) = req.oldest {
                            query.push(("oldest", oldest.clone()));
                        }
                        if let Some(ref latest) = req.latest {
                            query.push(("latest", latest.clone()));
                        }

                        rl.acquire().await;
                        match hc
                            .get("https://slack.com/api/conversations.history")
                            .bearer_auth(&bt)
                            .query(&query)
                            .send()
                            .await
                        {
                            Ok(resp) => match resp.json::<serde_json::Value>().await {
                                Ok(api_resp) if api_resp["ok"].as_bool().unwrap_or(false) => {
                                    let messages: Vec<SlackReadMessage> = api_resp["messages"]
                                        .as_array()
                                        .unwrap_or(&vec![])
                                        .iter()
                                        .map(|m| SlackReadMessage {
                                            ts: m["ts"].as_str().unwrap_or("").to_string(),
                                            user: m["user"].as_str().map(String::from),
                                            text: m["text"].as_str().map(String::from),
                                            bot_id: m["bot_id"].as_str().map(String::from),
                                        })
                                        .collect();
                                    let response = SlackReadMessagesResponse { ok: true, messages, error: None };
                                    match serde_json::to_vec(&response) {
                                        Ok(bytes) => {
                                            if let Err(e) = nc.publish(reply_to, bytes.into()).await {
                                                tracing::error!(
                                                    error = %e,
                                                    "Failed to publish read_messages reply"
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!(
                                                error = %e,
                                                "Failed to serialize read_messages response"
                                            );
                                        }
                                    }
                                }
                                Ok(api_resp) => {
                                    tracing::error!(
                                        api_error = api_resp["error"].as_str().unwrap_or("unknown"),
                                        "conversations.history failed"
                                    );
                                    let response = SlackReadMessagesResponse {
                                        ok: false,
                                        messages: vec![],
                                        error: Some(api_resp["error"].as_str().unwrap_or("unknown").to_string()),
                                    };
                                    if let Ok(bytes) = serde_json::to_vec(&response) {
                                        let _ = nc.publish(reply_to, bytes.into()).await;
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(
                                        error = %e,
                                        "Failed to parse conversations.history response"
                                    );
                                }
                            },
                            Err(e) => {
                                tracing::error!(
                                    error = %e,
                                    "HTTP error calling conversations.history"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "Failed to deserialize read_messages NATS message"
                        );
                    }
                }
            }
        }
    });
    let read_messages_abort = read_messages_handle.abort_handle();

    // read_replies handler: Core NATS request/reply — stays on raw client.
    let read_replies_handle = tokio::spawn({
        let nc = nats_client.clone();
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move {
            while let Some(msg) = read_replies_sub.next().await {
                let reply_to = match msg.reply {
                    Some(ref r) => r.clone(),
                    None => {
                        tracing::warn!("read_replies message has no reply subject, ignoring");
                        continue;
                    }
                };

                match serde_json::from_slice::<SlackReadRepliesRequest>(&msg.payload) {
                    Ok(req) => {
                        let limit = req.limit.unwrap_or(20).min(200);
                        let mut query: Vec<(&str, String)> = vec![
                            ("channel", req.channel.clone()),
                            ("ts", req.ts.clone()),
                            ("limit", limit.to_string()),
                        ];
                        if let Some(ref oldest) = req.oldest {
                            query.push(("oldest", oldest.clone()));
                        }
                        if let Some(ref latest) = req.latest {
                            query.push(("latest", latest.clone()));
                        }

                        rl.acquire().await;
                        match hc
                            .get("https://slack.com/api/conversations.replies")
                            .bearer_auth(&bt)
                            .query(&query)
                            .send()
                            .await
                        {
                            Ok(resp) => match resp.json::<serde_json::Value>().await {
                                Ok(api_resp) if api_resp["ok"].as_bool().unwrap_or(false) => {
                                    let messages: Vec<SlackReadMessage> = api_resp["messages"]
                                        .as_array()
                                        .unwrap_or(&vec![])
                                        .iter()
                                        .map(|m| SlackReadMessage {
                                            ts: m["ts"].as_str().unwrap_or("").to_string(),
                                            user: m["user"].as_str().map(String::from),
                                            text: m["text"].as_str().map(String::from),
                                            bot_id: m["bot_id"].as_str().map(String::from),
                                        })
                                        .collect();
                                    let response = SlackReadRepliesResponse {
                                        ok: true,
                                        messages,
                                        error: None,
                                    };
                                    match serde_json::to_vec(&response) {
                                        Ok(bytes) => {
                                            if let Err(e) = nc.publish(reply_to, bytes.into()).await {
                                                tracing::error!(
                                                    error = %e,
                                                    "Failed to publish read_replies reply"
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!(
                                                error = %e,
                                                "Failed to serialize read_replies response"
                                            );
                                        }
                                    }
                                }
                                Ok(api_resp) => {
                                    tracing::error!(
                                        api_error = api_resp["error"].as_str().unwrap_or("unknown"),
                                        "conversations.replies failed"
                                    );
                                    let response = SlackReadRepliesResponse {
                                        ok: false,
                                        messages: vec![],
                                        error: Some(
                                            api_resp["error"]
                                                .as_str()
                                                .unwrap_or("unknown")
                                                .to_string(),
                                        ),
                                    };
                                    if let Ok(bytes) = serde_json::to_vec(&response) {
                                        let _ = nc.publish(reply_to, bytes.into()).await;
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(
                                        error = %e,
                                        "Failed to parse conversations.replies response"
                                    );
                                }
                            },
                            Err(e) => {
                                tracing::error!(
                                    error = %e,
                                    "HTTP error calling conversations.replies"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "Failed to deserialize read_replies NATS message"
                        );
                    }
                }
            }
        }
    });
    let read_replies_abort = read_replies_handle.abort_handle();

    let suggested_prompts_handle = tokio::spawn({
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move { run_suggested_prompts_loop(suggested_prompts_consumer, bt, hc, rl).await }
    });
    let suggested_prompts_abort = suggested_prompts_handle.abort_handle();

    let proactive_handle = tokio::spawn({
        let sc = slack_client.clone();
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move { run_proactive_loop(proactive_consumer, sc, bt, hc, rl).await }
    });
    let proactive_abort = proactive_handle.abort_handle();

    let ephemeral_handle = tokio::spawn({
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move { run_ephemeral_loop(ephemeral_consumer, bt, hc, rl).await }
    });
    let ephemeral_abort = ephemeral_handle.abort_handle();

    let delete_file_handle = tokio::spawn({
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move { run_delete_file_loop(delete_file_consumer, bt, hc, rl).await }
    });
    let delete_file_abort = delete_file_handle.abort_handle();


    // list_users handler: Core NATS request/reply — stays on raw client.
    let list_users_handle = tokio::spawn({
        let nc = nats_client.clone();
        let bt = bot_token.clone();
        let user_tok = config.user_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move {
            while let Some(msg) = list_users_sub.next().await {
                let reply_to = match msg.reply {
                    Some(ref r) => r.clone(),
                    None => {
                        tracing::warn!("list_users message has no reply subject, ignoring");
                        continue;
                    }
                };

                match serde_json::from_slice::<SlackListUsersRequest>(&msg.payload) {
                    Ok(req) => {
                        let limit = req.limit.unwrap_or(200).min(200);
                        let mut query: Vec<(&str, String)> = vec![
                            ("limit", limit.to_string()),
                        ];
                        if let Some(ref cursor) = req.cursor {
                            query.push(("cursor", cursor.clone()));
                        }

                        rl.acquire().await;
                        let auth_token_users = user_tok.as_deref().unwrap_or(&bt);
                        match hc
                            .get("https://slack.com/api/users.list")
                            .bearer_auth(auth_token_users)
                            .query(&query)
                            .send()
                            .await
                        {
                            Ok(resp) => match resp.json::<serde_json::Value>().await {
                                Ok(api_resp) if api_resp["ok"].as_bool().unwrap_or(false) => {
                                    let members: Vec<SlackListUsersUser> = api_resp["members"]
                                        .as_array()
                                        .unwrap_or(&vec![])
                                        .iter()
                                        .map(|m| SlackListUsersUser {
                                            id: m["id"].as_str().unwrap_or("").to_string(),
                                            name: m["name"].as_str().unwrap_or("").to_string(),
                                            real_name: m["profile"]["real_name"].as_str().map(String::from),
                                            display_name: m["profile"]["display_name"].as_str().map(String::from),
                                            is_bot: m["is_bot"].as_bool().unwrap_or(false),
                                            deleted: m["deleted"].as_bool().unwrap_or(false),
                                        })
                                        .collect();
                                    let next_cursor = api_resp["response_metadata"]["next_cursor"]
                                        .as_str()
                                        .filter(|s| !s.is_empty())
                                        .map(String::from);
                                    let response = SlackListUsersResponse {
                                        ok: true,
                                        members,
                                        next_cursor,
                                        error: None,
                                    };
                                    match serde_json::to_vec(&response) {
                                        Ok(bytes) => {
                                            if let Err(e) = nc.publish(reply_to, bytes.into()).await {
                                                tracing::error!(
                                                    error = %e,
                                                    "Failed to publish list_users reply"
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!(
                                                error = %e,
                                                "Failed to serialize list_users response"
                                            );
                                        }
                                    }
                                }
                                Ok(api_resp) => {
                                    tracing::error!(
                                        api_error = api_resp["error"].as_str().unwrap_or("unknown"),
                                        "users.list failed"
                                    );
                                    let response = SlackListUsersResponse {
                                        ok: false,
                                        members: vec![],
                                        next_cursor: None,
                                        error: Some(api_resp["error"].as_str().unwrap_or("unknown").to_string()),
                                    };
                                    if let Ok(bytes) = serde_json::to_vec(&response) {
                                        let _ = nc.publish(reply_to, bytes.into()).await;
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(
                                        error = %e,
                                        "Failed to parse users.list response"
                                    );
                                }
                            },
                            Err(e) => {
                                tracing::error!(
                                    error = %e,
                                    "HTTP error calling users.list"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "Failed to deserialize list_users NATS message"
                        );
                    }
                }
            }
        }
    });
    let list_users_abort = list_users_handle.abort_handle();

    // list_conversations handler: Core NATS request/reply — stays on raw client.
    let list_conversations_handle = tokio::spawn({
        let nc = nats_client.clone();
        let bt = bot_token.clone();
        let user_tok = config.user_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move {
            while let Some(msg) = list_conversations_sub.next().await {
                let reply_to = match msg.reply {
                    Some(ref r) => r.clone(),
                    None => {
                        tracing::warn!("list_conversations message has no reply subject, ignoring");
                        continue;
                    }
                };

                match serde_json::from_slice::<SlackListConversationsRequest>(&msg.payload) {
                    Ok(req) => {
                        let limit = req.limit.unwrap_or(200).min(1000);
                        let mut query: Vec<(&str, String)> = vec![
                            ("limit", limit.to_string()),
                        ];
                        if let Some(ref cursor) = req.cursor {
                            query.push(("cursor", cursor.clone()));
                        }
                        if let Some(exclude_archived) = req.exclude_archived {
                            query.push(("exclude_archived", exclude_archived.to_string()));
                        }
                        if let Some(ref types) = req.types {
                            query.push(("types", types.clone()));
                        }

                        rl.acquire().await;
                        let auth_token_conv = user_tok.as_deref().unwrap_or(&bt);
                        match hc
                            .get("https://slack.com/api/conversations.list")
                            .bearer_auth(auth_token_conv)
                            .query(&query)
                            .send()
                            .await
                        {
                            Ok(resp) => match resp.json::<serde_json::Value>().await {
                                Ok(api_resp) if api_resp["ok"].as_bool().unwrap_or(false) => {
                                    let channels: Vec<SlackListConversationsChannel> = api_resp["channels"]
                                        .as_array()
                                        .unwrap_or(&vec![])
                                        .iter()
                                        .map(|c| SlackListConversationsChannel {
                                            id: c["id"].as_str().unwrap_or("").to_string(),
                                            name: c["name"].as_str().map(String::from),
                                            is_channel: c["is_channel"].as_bool().unwrap_or(false),
                                            is_private: c["is_private"].as_bool().unwrap_or(false),
                                            is_archived: c["is_archived"].as_bool().unwrap_or(false),
                                            num_members: c["num_members"].as_u64().map(|n| n as u32),
                                        })
                                        .collect();
                                    let next_cursor = api_resp["response_metadata"]["next_cursor"]
                                        .as_str()
                                        .filter(|s| !s.is_empty())
                                        .map(String::from);
                                    let response = SlackListConversationsResponse {
                                        ok: true,
                                        channels,
                                        next_cursor,
                                        error: None,
                                    };
                                    match serde_json::to_vec(&response) {
                                        Ok(bytes) => {
                                            if let Err(e) = nc.publish(reply_to, bytes.into()).await {
                                                tracing::error!(
                                                    error = %e,
                                                    "Failed to publish list_conversations reply"
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!(
                                                error = %e,
                                                "Failed to serialize list_conversations response"
                                            );
                                        }
                                    }
                                }
                                Ok(api_resp) => {
                                    tracing::error!(
                                        api_error = api_resp["error"].as_str().unwrap_or("unknown"),
                                        "conversations.list failed"
                                    );
                                    let response = SlackListConversationsResponse {
                                        ok: false,
                                        channels: vec![],
                                        next_cursor: None,
                                        error: Some(api_resp["error"].as_str().unwrap_or("unknown").to_string()),
                                    };
                                    if let Ok(bytes) = serde_json::to_vec(&response) {
                                        let _ = nc.publish(reply_to, bytes.into()).await;
                                    }
                                }
                                Err(e) => {
                                    tracing::error!(
                                        error = %e,
                                        "Failed to parse conversations.list response"
                                    );
                                }
                            },
                            Err(e) => {
                                tracing::error!(
                                    error = %e,
                                    "HTTP error calling conversations.list"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "Failed to deserialize list_conversations NATS message"
                        );
                    }
                }
            }
        }
    });
    let list_conversations_abort = list_conversations_handle.abort_handle();

    tokio::select! {
        _ = socket_mode_listener.serve() => {
            tracing::warn!("Socket Mode listener exited");
        }
        res = outbound_handle => {
            match res {
                Ok(()) => tracing::warn!("Outbound NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Outbound NATS loop panicked"),
            }
        }
        res = stream_append_handle => {
            match res {
                Ok(()) => tracing::warn!("Stream append NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Stream append NATS loop panicked"),
            }
        }
        res = stream_stop_handle => {
            match res {
                Ok(()) => tracing::warn!("Stream stop NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Stream stop NATS loop panicked"),
            }
        }
        res = stream_start_handle => {
            match res {
                Ok(()) => tracing::warn!("Stream start NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Stream start NATS loop panicked"),
            }
        }
        res = read_messages_handle => {
            match res {
                Ok(()) => tracing::warn!("Read messages NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Read messages NATS loop panicked"),
            }
        }
        res = reaction_action_handle => {
            match res {
                Ok(()) => tracing::warn!("Reaction action NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Reaction action NATS loop panicked"),
            }
        }
        res = view_open_handle => {
            match res {
                Ok(()) => tracing::warn!("View open NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "View open NATS loop panicked"),
            }
        }
        res = view_publish_handle => {
            match res {
                Ok(()) => tracing::warn!("View publish NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "View publish NATS loop panicked"),
            }
        }
        res = set_status_handle => {
            match res {
                Ok(()) => tracing::warn!("Set status NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Set status NATS loop panicked"),
            }
        }
        res = delete_handle => {
            match res {
                Ok(()) => tracing::warn!("Delete NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Delete NATS loop panicked"),
            }
        }
        res = update_handle => {
            match res {
                Ok(()) => tracing::warn!("Update NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Update NATS loop panicked"),
            }
        }
        res = upload_handle => {
            match res {
                Ok(()) => tracing::warn!("Upload NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Upload NATS loop panicked"),
            }
        }
        res = read_replies_handle => {
            match res {
                Ok(()) => tracing::warn!("Read replies NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Read replies NATS loop panicked"),
            }
        }
        res = suggested_prompts_handle => {
            match res {
                Ok(()) => tracing::warn!("Suggested prompts NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Suggested prompts NATS loop panicked"),
            }
        }
        res = proactive_handle => {
            match res {
                Ok(()) => tracing::warn!("Proactive NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Proactive NATS loop panicked"),
            }
        }
        res = ephemeral_handle => {
            match res {
                Ok(()) => tracing::warn!("Ephemeral NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Ephemeral NATS loop panicked"),
            }
        }
        res = delete_file_handle => {
            match res {
                Ok(()) => tracing::warn!("Delete file NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Delete file NATS loop panicked"),
            }
        }
        res = list_users_handle => {
            match res {
                Ok(()) => tracing::warn!("List users NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "List users NATS loop panicked"),
            }
        }
        res = list_conversations_handle => {
            match res {
                Ok(()) => tracing::warn!("List conversations NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "List conversations NATS loop panicked"),
            }
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received Ctrl+C, shutting down");
        }
    }

    // Abort any still-running NATS consumer loops and give them a moment to exit.
    outbound_abort.abort();
    stream_append_abort.abort();
    stream_stop_abort.abort();
    stream_start_abort.abort();
    read_messages_abort.abort();
    reaction_action_abort.abort();
    view_open_abort.abort();
    view_publish_abort.abort();
    set_status_abort.abort();
    delete_abort.abort();
    update_abort.abort();
    upload_abort.abort();
    read_replies_abort.abort();
    suggested_prompts_abort.abort();
    proactive_abort.abort();
    ephemeral_abort.abort();
    delete_file_abort.abort();
    list_users_abort.abort();
    list_conversations_abort.abort();
    tokio::time::sleep(Duration::from_millis(200)).await;
    tracing::info!("Shutdown complete");

    Ok(())
}
