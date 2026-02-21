mod config;
mod format;
mod health;
mod listener;
mod rate_limit;
mod sender;
mod webhook;

use async_nats::jetstream;
use config::{BotMode, SlackBotConfig};
use futures::StreamExt;
use listener::{
    BotState, error_handler, handle_command_event, handle_interaction_event, handle_push_event,
};
use sender::{
    run_delete_file_loop, run_delete_loop, run_ephemeral_loop, run_outbound_loop,
    run_proactive_loop, run_reaction_action_loop, run_set_status_loop, run_stream_append_loop,
    run_stream_stop_loop, run_suggested_prompts_loop, run_unfurl_loop, run_update_loop,
    run_upload_loop, run_view_open_loop, run_view_publish_loop,
};
use slack_morphism::prelude::*;
use slack_nats::setup::ensure_slack_stream;
use slack_nats::subscriber::{
    create_delete_consumer, create_delete_file_consumer, create_ephemeral_consumer,
    create_outbound_consumer, create_proactive_consumer, create_reaction_action_consumer,
    create_set_status_consumer, create_stream_append_consumer, create_stream_stop_consumer,
    create_suggested_prompts_consumer, create_unfurl_consumer, create_update_consumer,
    create_upload_consumer, create_view_open_consumer, create_view_publish_consumer,
};
use rate_limit::RateLimiter;
use slack_types::events::{
    SlackGetEmojiRequest, SlackGetEmojiResponse, SlackGetUserRequest, SlackGetUserResponse,
    SlackListConversationsChannel, SlackListConversationsRequest, SlackListConversationsResponse,
    SlackListUsersRequest, SlackListUsersResponse, SlackListUsersUser, SlackReadMessage,
    SlackReadMessagesRequest, SlackReadMessagesResponse, SlackReadRepliesRequest,
    SlackReadRepliesResponse, SlackStreamStartRequest, SlackStreamStartResponse,
};
use slack_types::subjects::{
    for_account, SLACK_OUTBOUND_GET_EMOJI, SLACK_OUTBOUND_GET_USER,
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
    let account_id = config.account_id.as_deref();
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
        config.http_path.clone(),
        config.signing_secret.clone(),
        js.clone(),
        config.bot_token.clone(),
        config.bot_user_id.clone(),
        config.allow_bots,
        config.media_max_mb,
        config.mention_gating,
        config.mention_gating_channels.clone(),
        config.no_mention_channels.clone(),
        config.mention_patterns.clone(),
        config.account_id.clone(),
    ));

    tracing::info!("Creating JetStream consumers...");
    let outbound_consumer = create_outbound_consumer(&js, account_id).await?;
    let stream_append_consumer = create_stream_append_consumer(&js, account_id).await?;
    let stream_stop_consumer = create_stream_stop_consumer(&js, account_id).await?;
    let reaction_action_consumer = create_reaction_action_consumer(&js, account_id).await?;
    let view_open_consumer = create_view_open_consumer(&js, account_id).await?;
    let view_publish_consumer = create_view_publish_consumer(&js, account_id).await?;
    let set_status_consumer = create_set_status_consumer(&js, account_id).await?;
    let delete_consumer = create_delete_consumer(&js, account_id).await?;
    let update_consumer = create_update_consumer(&js, account_id).await?;
    let upload_consumer = create_upload_consumer(&js, account_id).await?;
    let suggested_prompts_consumer = create_suggested_prompts_consumer(&js, account_id).await?;
    let proactive_consumer = create_proactive_consumer(&js, account_id).await?;
    let ephemeral_consumer = create_ephemeral_consumer(&js, account_id).await?;
    let delete_file_consumer = create_delete_file_consumer(&js, account_id).await?;
    let unfurl_consumer = create_unfurl_consumer(&js, account_id).await?;

    // stream.start uses Core NATS request/reply — subscribe on the raw client.
    let mut stream_start_sub = nats_client.subscribe(for_account(SLACK_OUTBOUND_STREAM_START, account_id)).await?;
    let mut read_messages_sub = nats_client.subscribe(for_account(SLACK_OUTBOUND_READ_MESSAGES, account_id)).await?;
    let mut read_replies_sub = nats_client.subscribe(for_account(SLACK_OUTBOUND_READ_REPLIES, account_id)).await?;
    let mut list_users_sub = nats_client.subscribe(for_account(SLACK_OUTBOUND_LIST_USERS, account_id)).await?;
    let mut list_conversations_sub = nats_client.subscribe(for_account(SLACK_OUTBOUND_LIST_CONVERSATIONS, account_id)).await?;
    let mut get_user_sub = nats_client.subscribe(for_account(SLACK_OUTBOUND_GET_USER, account_id)).await?;
    let mut get_emoji_sub = nats_client.subscribe(for_account(SLACK_OUTBOUND_GET_EMOJI, account_id)).await?;

    // slack_client is needed for outbound/proactive loops in both modes.
    let slack_client = Arc::new(SlackClient::new(SlackClientHyperConnector::new()?));

    // HTTP mode: the webhook server was already tokio::spawned above.
    // Socket Mode: set up BotState, callbacks, listener, and connect.
    // We use a boxed future to unify both cases in tokio::select!.
    let slack_listener_fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        if config.mode == BotMode::Socket {
            tracing::info!("Connecting to Slack via Socket Mode...");

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
                account_id: config.account_id.clone(),
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

            let app_token_str = config
                .app_token
                .as_deref()
                .expect("SLACK_APP_TOKEN is required in Socket Mode")
                .to_string();
            let app_token: SlackApiToken = SlackApiToken::new(app_token_str.into());
            socket_mode_listener.listen_for(&app_token).await?;

            tracing::info!("Slack bot running (Socket Mode). Press Ctrl+C to stop.");

            Box::pin(async move { let _ = socket_mode_listener.serve().await; })
        } else {
            tracing::info!(
                "Running in HTTP Events API mode. Slack events received via webhook on port {}.",
                config.events_port
            );
            // HTTP mode: the webhook server is already spawned above.
            // Just wait forever (Ctrl+C will terminate).
            Box::pin(std::future::pending())
        };

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
                        let subject = for_account("slack.bot.identity", account_id);
                        let _ = nats_client
                            .publish(subject, payload.into())
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

    let unfurl_handle = tokio::spawn({
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move { run_unfurl_loop(unfurl_consumer, bt, hc, rl).await }
    });
    let unfurl_abort = unfurl_handle.abort_handle();

    // get_user handler: Core NATS request/reply — calls users.info.
    let get_user_handle = tokio::spawn({
        let nc = nats_client.clone();
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move {
            while let Some(msg) = get_user_sub.next().await {
                let reply_to = match msg.reply {
                    Some(ref r) => r.clone(),
                    None => {
                        tracing::warn!("get_user message has no reply subject, ignoring");
                        continue;
                    }
                };

                match serde_json::from_slice::<SlackGetUserRequest>(&msg.payload) {
                    Ok(req) => {
                        let url = format!("https://slack.com/api/users.info?user={}", req.user_id);
                        rl.acquire().await;
                        match hc.get(&url).bearer_auth(&bt).send().await {
                            Ok(resp) => match resp.json::<serde_json::Value>().await {
                                Ok(api_resp) if api_resp["ok"].as_bool().unwrap_or(false) => {
                                    let u = &api_resp["user"];
                                    let user = SlackListUsersUser {
                                        id: u["id"].as_str().unwrap_or("").to_string(),
                                        name: u["name"].as_str().unwrap_or("").to_string(),
                                        real_name: u["real_name"].as_str().map(String::from),
                                        display_name: u["profile"]["display_name"]
                                            .as_str()
                                            .filter(|s| !s.is_empty())
                                            .or_else(|| u["profile"]["real_name"].as_str())
                                            .map(String::from),
                                        is_bot: u["is_bot"].as_bool().unwrap_or(false),
                                        deleted: u["deleted"].as_bool().unwrap_or(false),
                                    };
                                    let response = SlackGetUserResponse {
                                        ok: true,
                                        user: Some(user),
                                        error: None,
                                    };
                                    if let Ok(bytes) = serde_json::to_vec(&response) {
                                        let _ = nc.publish(reply_to, bytes.into()).await;
                                    }
                                }
                                Ok(api_resp) => {
                                    tracing::warn!(
                                        api_error = api_resp["error"].as_str().unwrap_or("unknown"),
                                        user_id = %req.user_id,
                                        "users.info failed"
                                    );
                                    let response = SlackGetUserResponse {
                                        ok: false,
                                        user: None,
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
                                    tracing::error!(error = %e, "Failed to parse users.info response");
                                }
                            },
                            Err(e) => {
                                tracing::error!(error = %e, "HTTP error calling users.info");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to deserialize SlackGetUserRequest");
                    }
                }
            }
        }
    });
    let get_user_abort = get_user_handle.abort_handle();

    // get_emoji handler: Core NATS request/reply — calls emoji.list.
    let get_emoji_handle = tokio::spawn({
        let nc = nats_client.clone();
        let bt = bot_token.clone();
        let hc = Arc::new(reqwest::Client::new());
        let rl = rate_limiter.clone();
        async move {
            while let Some(msg) = get_emoji_sub.next().await {
                let reply_to = match msg.reply {
                    Some(ref r) => r.clone(),
                    None => {
                        tracing::warn!("get_emoji message has no reply subject, ignoring");
                        continue;
                    }
                };

                // Deserialize request (empty struct — just a trigger).
                let _req = match serde_json::from_slice::<SlackGetEmojiRequest>(&msg.payload) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to deserialize SlackGetEmojiRequest");
                        continue;
                    }
                };

                rl.acquire().await;
                match hc
                    .get("https://slack.com/api/emoji.list")
                    .bearer_auth(&bt)
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(api_resp) if api_resp["ok"].as_bool().unwrap_or(false) => {
                            let emoji: std::collections::HashMap<String, String> = api_resp
                                ["emoji"]
                                .as_object()
                                .map(|obj| {
                                    obj.iter()
                                        .filter_map(|(k, v)| {
                                            v.as_str().map(|s| (k.clone(), s.to_string()))
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            let response = SlackGetEmojiResponse { ok: true, emoji, error: None };
                            if let Ok(bytes) = serde_json::to_vec(&response) {
                                let _ = nc.publish(reply_to, bytes.into()).await;
                            }
                        }
                        Ok(api_resp) => {
                            tracing::warn!(
                                api_error = api_resp["error"].as_str().unwrap_or("unknown"),
                                "emoji.list failed"
                            );
                            let response = SlackGetEmojiResponse {
                                ok: false,
                                emoji: Default::default(),
                                error: Some(
                                    api_resp["error"].as_str().unwrap_or("unknown").to_string(),
                                ),
                            };
                            if let Ok(bytes) = serde_json::to_vec(&response) {
                                let _ = nc.publish(reply_to, bytes.into()).await;
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to parse emoji.list response");
                        }
                    },
                    Err(e) => {
                        tracing::error!(error = %e, "HTTP error calling emoji.list");
                    }
                }
            }
        }
    });
    let get_emoji_abort = get_emoji_handle.abort_handle();

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
        _ = slack_listener_fut => {
            tracing::warn!("Slack listener exited");
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
        res = unfurl_handle => {
            match res {
                Ok(()) => tracing::warn!("Unfurl NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Unfurl NATS loop panicked"),
            }
        }
        res = get_user_handle => {
            match res {
                Ok(()) => tracing::warn!("Get user NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Get user NATS loop panicked"),
            }
        }
        res = get_emoji_handle => {
            match res {
                Ok(()) => tracing::warn!("Get emoji NATS loop exited"),
                Err(e) => tracing::error!(error = %e, "Get emoji NATS loop panicked"),
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
    unfurl_abort.abort();
    get_user_abort.abort();
    get_emoji_abort.abort();
    tokio::time::sleep(Duration::from_millis(200)).await;
    tracing::info!("Shutdown complete");

    Ok(())
}
