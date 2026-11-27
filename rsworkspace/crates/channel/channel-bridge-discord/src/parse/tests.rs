use super::*;
use channel_acp_port::sender_endpoint;
use trogon_channel::Command;

const BOT: u64 = 900;
const USER: u64 = 42;
const DM_CHANNEL: u64 = 7001;
const GUILD_CHANNEL: u64 = 8001;

fn account() -> ChannelAccount {
    ChannelAccount::new("discord", "mybot").expect("valid account")
}

fn bot() -> Id<UserMarker> {
    Id::new(BOT)
}

/// A `MESSAGE_CREATE` body as the gateway publishes it: Discord's own JSON,
/// snowflakes as strings. Parsed from bytes, as the pipeline does.
fn message(content: &str, guild: bool, author_is_bot: bool, mentions_bot: bool) -> Message {
    let mut body = serde_json::json!({
        "id": "1234",
        "channel_id": if guild { GUILD_CHANNEL.to_string() } else { DM_CHANNEL.to_string() },
        "author": {
            "id": USER.to_string(),
            "username": "pat",
            "global_name": "Pat",
            "discriminator": "0",
            "avatar": null,
            "bot": author_is_bot,
        },
        "content": content,
        "timestamp": "2026-11-20T10:00:00.000000+00:00",
        "edited_timestamp": null,
        "tts": false,
        "mention_everyone": false,
        "mentions": [],
        "mention_roles": [],
        "attachments": [],
        "embeds": [],
        "pinned": false,
        "type": 0,
    });
    if guild {
        body["guild_id"] = serde_json::json!("5001");
    }
    if mentions_bot {
        body["mentions"] = serde_json::json!([{
            "id": BOT.to_string(),
            "username": "trogon",
            "discriminator": "0",
            "avatar": null,
            "bot": true,
            "public_flags": 0,
        }]);
    }
    let bytes = serde_json::to_vec(&body).expect("serialize");
    serde_json::from_slice(&bytes).expect("a MESSAGE_CREATE body deserializes")
}

#[test]
fn a_direct_message_is_keyed_on_the_user_and_answered_in_the_dm_channel() {
    let inbound = inbound(
        &message("fix the build", false, false, false),
        bot(),
        &account(),
        &CommandTriggers::default(),
    )
    .expect("carried");

    assert_eq!(inbound.event.endpoint.peer(), USER.to_string());
    assert_eq!(inbound.reply_to.get(), DM_CHANNEL);
    assert_eq!(inbound.event.text.as_deref(), Some("fix the build"));
    assert_eq!(inbound.event.sender.display_name, "Pat");
    assert_eq!(inbound.event.message_ref.as_str(), "1234");
}

#[test]
fn a_guild_message_that_mentions_the_bot_is_keyed_on_the_channel_without_the_mention() {
    let inbound = inbound(
        &message(&format!("<@{BOT}> fix the build"), true, false, true),
        bot(),
        &account(),
        &CommandTriggers::default(),
    )
    .expect("carried");

    assert_eq!(inbound.event.endpoint.peer(), GUILD_CHANNEL.to_string());
    assert_eq!(inbound.reply_to.get(), GUILD_CHANNEL);
    assert_eq!(inbound.event.text.as_deref(), Some("fix the build"));
}

#[test]
fn the_nickname_mention_form_is_stripped_too() {
    let inbound = inbound(
        &message(&format!("<@!{BOT}> hi"), true, false, true),
        bot(),
        &account(),
        &CommandTriggers::default(),
    )
    .expect("carried");

    assert_eq!(inbound.event.text.as_deref(), Some("hi"));
}

#[test]
fn a_guild_message_that_does_not_mention_the_bot_is_not_carried() {
    let carried = inbound(
        &message("just chatting", true, false, false),
        bot(),
        &account(),
        &CommandTriggers::default(),
    );
    assert!(carried.is_none());
}

#[test]
fn messages_from_bots_are_never_carried() {
    let carried = inbound(
        &message("echo", false, true, false),
        bot(),
        &account(),
        &CommandTriggers::default(),
    );
    assert!(carried.is_none());
}

#[test]
fn a_bare_mention_with_nothing_said_is_not_carried() {
    let carried = inbound(
        &message(&format!("<@{BOT}>"), true, false, true),
        bot(),
        &account(),
        &CommandTriggers::default(),
    );
    assert!(carried.is_none());
}

#[test]
fn a_new_session_trigger_is_read_as_a_command() {
    let inbound = inbound(
        &message("/new", false, false, false),
        bot(),
        &account(),
        &CommandTriggers::default(),
    )
    .expect("carried");

    assert_eq!(inbound.event.command, Some(Command::NewSession));
    assert_eq!(inbound.event.text, None);
}

#[test]
fn the_sender_endpoint_is_the_user_even_in_a_guild_channel() {
    let inbound = inbound(
        &message(&format!("<@{BOT}> hi"), true, false, true),
        bot(),
        &account(),
        &CommandTriggers::default(),
    )
    .expect("carried");

    let sender = sender_endpoint(&account(), &inbound.event.sender);
    assert_eq!(sender.peer(), USER.to_string());
    assert_ne!(sender, inbound.event.endpoint);
}
