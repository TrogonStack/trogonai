//! Message handlers for Telegram updates

use teloxide::prelude::*;
use teloxide::types::{Message, CallbackQuery};
use tracing::{debug, error, info, warn};

use crate::bridge::TelegramBridge;
use crate::health::AppState;

/// Handle text messages
pub async fn handle_text_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let text = msg.text().unwrap_or_default();
    let update_id = msg.id.0 as i64;

    debug!("Received text message: {}", text);

    // Increment metrics
    health.increment_messages_received().await;

    // Check for commands
    if text.starts_with('/') {
        return handle_command(msg, bridge, health, update_id).await;
    }

    // Check access control
    if !check_access(&msg, &bridge) {
        warn!("Access denied for user {} in chat {}",
            msg.from.as_ref().map(|u| u.id.0).unwrap_or(0),
            msg.chat.id.0);
        return Ok(());
    }

    // Publish to NATS
    if let Err(e) = bridge.publish_text_message(&msg, update_id).await {
        error!("Failed to publish text message: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle photo messages
pub async fn handle_photo_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received photo message");
    health.increment_messages_received().await;

    if !check_access(&msg, &bridge) {
        warn!("Access denied for photo message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_photo_message(&msg, update_id).await {
        error!("Failed to publish photo message: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle video messages
pub async fn handle_video_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received video message");
    health.increment_messages_received().await;

    if !check_access(&msg, &bridge) {
        warn!("Access denied for video message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_video_message(&msg, update_id).await {
        error!("Failed to publish video message: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle audio messages
pub async fn handle_audio_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received audio message");
    health.increment_messages_received().await;

    if !check_access(&msg, &bridge) {
        warn!("Access denied for audio message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_audio_message(&msg, update_id).await {
        error!("Failed to publish audio message: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle document messages
pub async fn handle_document_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received document message");
    health.increment_messages_received().await;

    if !check_access(&msg, &bridge) {
        warn!("Access denied for document message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_document_message(&msg, update_id).await {
        error!("Failed to publish document message: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle voice messages
pub async fn handle_voice_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received voice message");
    health.increment_messages_received().await;

    if !check_access(&msg, &bridge) {
        warn!("Access denied for voice message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_voice_message(&msg, update_id).await {
        error!("Failed to publish voice message: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle location messages
pub async fn handle_location_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;
    debug!("Received location message");
    health.increment_messages_received().await;
    if !check_access(&msg, &bridge) { return Ok(()); }
    if let Err(e) = bridge.publish_location_message(&msg, update_id).await {
        error!("Failed to publish location message: {}", e);
        health.increment_errors().await;
    }
    Ok(())
}

/// Handle venue messages
pub async fn handle_venue_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;
    debug!("Received venue message");
    health.increment_messages_received().await;
    if !check_access(&msg, &bridge) { return Ok(()); }
    if let Err(e) = bridge.publish_venue_message(&msg, update_id).await {
        error!("Failed to publish venue message: {}", e);
        health.increment_errors().await;
    }
    Ok(())
}

/// Handle contact messages
pub async fn handle_contact_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;
    debug!("Received contact message");
    health.increment_messages_received().await;
    if !check_access(&msg, &bridge) { return Ok(()); }
    if let Err(e) = bridge.publish_contact_message(&msg, update_id).await {
        error!("Failed to publish contact message: {}", e);
        health.increment_errors().await;
    }
    Ok(())
}

/// Handle sticker messages
pub async fn handle_sticker_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received sticker message");
    health.increment_messages_received().await;

    if !check_access(&msg, &bridge) {
        warn!("Access denied for sticker message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_sticker_message(&msg, update_id).await {
        error!("Failed to publish sticker message: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle animation (GIF) messages
pub async fn handle_animation_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received animation message");
    health.increment_messages_received().await;

    if !check_access(&msg, &bridge) {
        warn!("Access denied for animation message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_animation_message(&msg, update_id).await {
        error!("Failed to publish animation message: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle video note messages (short round videos)
pub async fn handle_video_note_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received video note message");
    health.increment_messages_received().await;

    if !check_access(&msg, &bridge) {
        warn!("Access denied for video note message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_video_note_message(&msg, update_id).await {
        error!("Failed to publish video note message: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle callback queries (button clicks)
pub async fn handle_callback_query(_bot: Bot, query: CallbackQuery, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = query.id.parse::<i64>().unwrap_or(0);

    debug!("Received callback query: {:?}", query.data);
    health.increment_messages_received().await;

    // Check access
    let user_id = query.from.id.0 as i64;
    if !bridge.check_dm_access(user_id) && !bridge.access_config.is_admin(user_id) {
        warn!("Access denied for callback query from user {}", user_id);
        return Ok(());
    }

    if let Err(e) = bridge.publish_callback_query(&query, update_id).await {
        error!("Failed to publish callback query: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle commands
async fn handle_command(msg: Message, bridge: TelegramBridge, health: AppState, update_id: i64) -> ResponseResult<()> {
    let text = msg.text().unwrap_or_default();
    let parts: Vec<&str> = text.split_whitespace().collect();

    if parts.is_empty() {
        return Ok(());
    }

    let command = parts[0].trim_start_matches('/').to_lowercase();
    let args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();

    info!("Received command: {} with {} args", command, args.len());
    health.increment_commands().await;

    // Check access
    if !check_access(&msg, &bridge) {
        warn!("Access denied for command: {}", command);
        return Ok(());
    }

    // Publish command to NATS
    if let Err(e) = bridge.publish_command(&msg, &command, args, update_id).await {
        error!("Failed to publish command: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle inline queries (@bot query)
pub async fn handle_inline_query(_bot: Bot, query: teloxide::types::InlineQuery, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    use telegram_types::events::{InlineQueryEvent, Location};

    let update_id = 0; // Inline queries don't have update_id

    debug!("Received inline query: {}", query.query);
    health.increment_messages_received().await;

    // Check access control
    let user_id = query.from.id.0 as i64;
    if !bridge.access_config.is_admin(user_id) && !bridge.check_dm_access(user_id) {
        warn!("Access denied for inline query from user {}", user_id);
        return Ok(());
    }

    // Create session ID for inline queries (user-specific)
    let session_id = format!("tg-inline-{}", user_id);

    // Convert location if present
    let location = query.location.map(|loc| Location {
        longitude: loc.longitude,
        latitude: loc.latitude,
    });

    // Create inline query event
    let event = InlineQueryEvent {
        metadata: telegram_types::events::EventMetadata::new(session_id, update_id),
        inline_query_id: query.id.clone(),
        from: telegram_types::chat::User {
            id: user_id,
            is_bot: query.from.is_bot,
            first_name: query.from.first_name.clone(),
            last_name: query.from.last_name.clone(),
            username: query.from.username.clone(),
            language_code: query.from.language_code.clone(),
        },
        query: query.query.clone(),
        offset: query.offset.clone(),
        chat_type: query.chat_type.map(|ct| format!("{:?}", ct)),
        location,
    };

    // Publish to NATS
    if let Err(e) = bridge.publish_inline_query(&event).await {
        error!("Failed to publish inline query: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle chosen inline results
pub async fn handle_chosen_inline_result(_bot: Bot, result: teloxide::types::ChosenInlineResult, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    use telegram_types::events::ChosenInlineResultEvent;

    let update_id = 0;

    debug!("Inline result chosen: {}", result.result_id);
    health.increment_messages_received().await;

    let user_id = result.from.id.0 as i64;
    let session_id = format!("tg-inline-{}", user_id);

    let event = ChosenInlineResultEvent {
        metadata: telegram_types::events::EventMetadata::new(session_id, update_id),
        result_id: result.result_id.clone(),
        from: telegram_types::chat::User {
            id: user_id,
            is_bot: result.from.is_bot,
            first_name: result.from.first_name.clone(),
            last_name: result.from.last_name.clone(),
            username: result.from.username.clone(),
            language_code: result.from.language_code.clone(),
        },
        query: result.query.clone(),
        inline_message_id: result.inline_message_id.clone(),
    };

    // Publish to NATS
    if let Err(e) = bridge.publish_chosen_inline_result(&event).await {
        error!("Failed to publish chosen inline result: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle chat member updates (when other users join/leave/get banned)
pub async fn handle_chat_member_updated(
    _bot: Bot,
    update: teloxide::types::ChatMemberUpdated,
    bridge: TelegramBridge,
    health: AppState
) -> ResponseResult<()> {
    let update_id = 0; // Chat member updates don't have a standard update_id

    debug!("Chat member updated: {:?} -> {:?}",
        update.old_chat_member.kind,
        update.new_chat_member.kind
    );
    health.increment_messages_received().await;

    // Publish to NATS
    if let Err(e) = bridge.publish_chat_member_updated(&update, update_id).await {
        error!("Failed to publish chat member updated: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle my chat member updates (when bot's status changes in a chat)
pub async fn handle_my_chat_member_updated(
    _bot: Bot,
    update: teloxide::types::ChatMemberUpdated,
    bridge: TelegramBridge,
    health: AppState
) -> ResponseResult<()> {
    let update_id = 0;

    info!("Bot chat member status changed: {:?} -> {:?} in chat {}",
        update.old_chat_member.kind,
        update.new_chat_member.kind,
        update.chat.id
    );
    health.increment_messages_received().await;

    // Publish to NATS
    if let Err(e) = bridge.publish_my_chat_member_updated(&update, update_id).await {
        error!("Failed to publish my chat member updated: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle pre-checkout query (before payment is confirmed)
pub async fn handle_pre_checkout_query(
    _bot: Bot,
    query: teloxide::types::PreCheckoutQuery,
    bridge: TelegramBridge,
    health: AppState
) -> ResponseResult<()> {
    debug!("Pre-checkout query from user {}: {} {}",
        query.from.id,
        query.total_amount,
        query.currency
    );

    let update_id = 0; // Update ID is not available directly on PreCheckoutQuery
    health.increment_messages_received().await;

    if let Err(e) = bridge.publish_pre_checkout_query(&query, update_id).await {
        error!("Failed to publish pre-checkout query: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle successful payment
pub async fn handle_successful_payment(
    _bot: Bot,
    msg: Message,
    bridge: TelegramBridge,
    health: AppState
) -> ResponseResult<()> {
    if let Some(payment) = msg.successful_payment() {
        debug!("Successful payment: {} {} from chat {}",
            payment.total_amount,
            payment.currency,
            msg.chat.id
        );

        let update_id = msg.id.0 as i64;
        health.increment_messages_received().await;

        if let Err(e) = bridge.publish_successful_payment(&msg, payment, update_id).await {
            error!("Failed to publish successful payment: {}", e);
            health.increment_errors().await;
        }
    }

    Ok(())
}

/// Handle shipping query (for physical goods)
pub async fn handle_shipping_query(
    _bot: Bot,
    query: teloxide::types::ShippingQuery,
    bridge: TelegramBridge,
    health: AppState
) -> ResponseResult<()> {
    debug!("Shipping query from user {}", query.from.id);

    let update_id = 0;
    health.increment_messages_received().await;

    if let Err(e) = bridge.publish_shipping_query(&query, update_id).await {
        error!("Failed to publish shipping query: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle poll messages (poll sent inside a chat)
pub async fn handle_poll_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;

    debug!("Received poll message");
    health.increment_messages_received().await;

    if !check_access(&msg, &bridge) {
        warn!("Access denied for poll message");
        return Ok(());
    }

    if let Err(e) = bridge.publish_poll_message(&msg, update_id).await {
        error!("Failed to publish poll message: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle standalone poll updates (poll state changed)
pub async fn handle_poll_update(
    _bot: Bot,
    poll: teloxide::types::Poll,
    bridge: TelegramBridge,
    health: AppState,
) -> ResponseResult<()> {
    debug!("Poll update: {}", poll.id);
    health.increment_messages_received().await;

    if let Err(e) = bridge.publish_poll_update(&poll, 0).await {
        error!("Failed to publish poll update: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle poll answers (user voted in a non-anonymous poll)
pub async fn handle_poll_answer(
    _bot: Bot,
    answer: teloxide::types::PollAnswer,
    bridge: TelegramBridge,
    health: AppState,
) -> ResponseResult<()> {
    debug!("Poll answer for poll {}: {:?}", answer.poll_id, answer.option_ids);
    health.increment_messages_received().await;

    if let Err(e) = bridge.publish_poll_answer(&answer, 0).await {
        error!("Failed to publish poll answer: {}", e);
        health.increment_errors().await;
    }

    Ok(())
}

/// Handle edited messages (user edited a previously sent message)
pub async fn handle_edited_message(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;
    debug!("Received edited message");
    health.increment_messages_received().await;
    if !check_access(&msg, &bridge) { return Ok(()); }
    if let Err(e) = bridge.publish_edited_message(&msg, update_id).await {
        error!("Failed to publish edited message: {}", e);
        health.increment_errors().await;
    }
    Ok(())
}

/// Handle channel posts (messages sent to a channel the bot is in)
pub async fn handle_channel_post(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;
    debug!("Received channel post");
    health.increment_messages_received().await;

    // Channel posts have no user sender — check group access by chat ID
    let chat_id = msg.chat.id.0;
    if !bridge.access_config.is_admin(0) && !bridge.check_group_access(chat_id) {
        return Ok(());
    }

    // Route through the same bridge methods as regular messages; chat_type=channel
    // lets agents distinguish channel posts from group messages.
    if msg.text().is_some() {
        if let Err(e) = bridge.publish_text_message(&msg, update_id).await {
            error!("Failed to publish channel text post: {}", e);
            health.increment_errors().await;
        }
    } else if msg.photo().is_some() {
        if let Err(e) = bridge.publish_photo_message(&msg, update_id).await {
            error!("Failed to publish channel photo post: {}", e);
            health.increment_errors().await;
        }
    } else if msg.video().is_some() {
        if let Err(e) = bridge.publish_video_message(&msg, update_id).await {
            error!("Failed to publish channel video post: {}", e);
            health.increment_errors().await;
        }
    } else if msg.document().is_some() {
        if let Err(e) = bridge.publish_document_message(&msg, update_id).await {
            error!("Failed to publish channel document post: {}", e);
            health.increment_errors().await;
        }
    }
    Ok(())
}

/// Handle edited channel posts
pub async fn handle_edited_channel_post(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;
    debug!("Received edited channel post");
    health.increment_messages_received().await;

    let chat_id = msg.chat.id.0;
    if !bridge.check_group_access(chat_id) { return Ok(()); }

    if let Err(e) = bridge.publish_edited_message(&msg, update_id).await {
        error!("Failed to publish edited channel post: {}", e);
        health.increment_errors().await;
    }
    Ok(())
}

/// Handle chat join requests (user requested to join a group with approval required)
pub async fn handle_chat_join_request(
    _bot: Bot,
    request: teloxide::types::ChatJoinRequest,
    bridge: TelegramBridge,
    health: AppState,
) -> ResponseResult<()> {
    debug!("Chat join request from user {} in chat {}", request.from.id, request.chat.id);
    health.increment_messages_received().await;

    if let Err(e) = bridge.publish_chat_join_request(&request, 0).await {
        error!("Failed to publish chat join request: {}", e);
        health.increment_errors().await;
    }
    Ok(())
}

/// Handle forum topic created service messages
pub async fn handle_forum_topic_created(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;
    debug!("Forum topic created");
    health.increment_messages_received().await;
    if !check_access(&msg, &bridge) { return Ok(()); }
    if let Err(e) = bridge.publish_forum_topic_created(&msg, update_id).await {
        error!("Failed to publish forum topic created: {}", e);
        health.increment_errors().await;
    }
    Ok(())
}

/// Handle forum topic edited service messages
pub async fn handle_forum_topic_edited(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;
    debug!("Forum topic edited");
    health.increment_messages_received().await;
    if !check_access(&msg, &bridge) { return Ok(()); }
    if let Err(e) = bridge.publish_forum_topic_edited(&msg, update_id).await {
        error!("Failed to publish forum topic edited: {}", e);
        health.increment_errors().await;
    }
    Ok(())
}

/// Handle forum topic closed service messages
pub async fn handle_forum_topic_closed(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;
    debug!("Forum topic closed");
    health.increment_messages_received().await;
    if !check_access(&msg, &bridge) { return Ok(()); }
    if let Err(e) = bridge.publish_forum_topic_closed(&msg, update_id).await {
        error!("Failed to publish forum topic closed: {}", e);
        health.increment_errors().await;
    }
    Ok(())
}

/// Handle forum topic reopened service messages
pub async fn handle_forum_topic_reopened(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;
    debug!("Forum topic reopened");
    health.increment_messages_received().await;
    if !check_access(&msg, &bridge) { return Ok(()); }
    if let Err(e) = bridge.publish_forum_topic_reopened(&msg, update_id).await {
        error!("Failed to publish forum topic reopened: {}", e);
        health.increment_errors().await;
    }
    Ok(())
}

/// Handle general forum topic hidden service messages
pub async fn handle_general_forum_topic_hidden(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;
    debug!("General forum topic hidden");
    health.increment_messages_received().await;
    if !check_access(&msg, &bridge) { return Ok(()); }
    if let Err(e) = bridge.publish_general_forum_topic_hidden(&msg, update_id).await {
        error!("Failed to publish general forum topic hidden: {}", e);
        health.increment_errors().await;
    }
    Ok(())
}

/// Handle general forum topic unhidden service messages
pub async fn handle_general_forum_topic_unhidden(_bot: Bot, msg: Message, bridge: TelegramBridge, health: AppState) -> ResponseResult<()> {
    let update_id = msg.id.0 as i64;
    debug!("General forum topic unhidden");
    health.increment_messages_received().await;
    if !check_access(&msg, &bridge) { return Ok(()); }
    if let Err(e) = bridge.publish_general_forum_topic_unhidden(&msg, update_id).await {
        error!("Failed to publish general forum topic unhidden: {}", e);
        health.increment_errors().await;
    }
    Ok(())
}

/// Check if the message sender has access
fn check_access(msg: &Message, bridge: &TelegramBridge) -> bool {
    use teloxide::types::ChatKind;

    let user_id = msg.from.as_ref().map(|u| u.id.0 as i64).unwrap_or(0);

    // Admins always have access
    if bridge.access_config.is_admin(user_id) {
        return true;
    }

    match &msg.chat.kind {
        ChatKind::Private(_) => bridge.check_dm_access(user_id),
        ChatKind::Public(_) => {
            let chat_id = msg.chat.id.0;
            bridge.check_group_access(chat_id)
        }
    }
}
