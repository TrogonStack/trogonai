use super::*;

    #[test]
    fn parse_intents_csv() {
        let intents = parse_gateway_intents("guilds,guild_messages,message_content").unwrap();
        assert!(intents.contains(Intents::GUILDS));
        assert!(intents.contains(Intents::GUILD_MESSAGES));
        assert!(intents.contains(Intents::MESSAGE_CONTENT));
        assert!(!intents.contains(Intents::GUILD_PRESENCES));
    }

    #[test]
    fn parse_intents_all() {
        let intents = parse_gateway_intents("all").unwrap();
        assert_eq!(intents, Intents::all());
    }

    #[test]
    fn parse_intents_non_privileged() {
        let intents = parse_gateway_intents("non_privileged").unwrap();
        assert_eq!(intents, default_intents());
    }

    #[test]
    fn parse_intents_unknown_returns_error() {
        let err = parse_gateway_intents("guilds,bogus_intent,guild_messages").unwrap_err();
        assert_eq!(err.intent, "bogus_intent");
        assert_eq!(err.to_string(), "unknown gateway intent: 'bogus_intent'");
        assert!(std::error::Error::source(&err).is_none());
    }

    #[test]
    fn default_intents_excludes_privileged() {
        let intents = default_intents();
        assert!(!intents.contains(Intents::GUILD_MEMBERS));
        assert!(!intents.contains(Intents::GUILD_PRESENCES));
        assert!(!intents.contains(Intents::MESSAGE_CONTENT));
        assert!(intents.contains(Intents::GUILDS));
        assert!(intents.contains(Intents::GUILD_MESSAGES));
        assert!(intents.contains(Intents::GUILD_SCHEDULED_EVENTS));
    }

    #[test]
    fn parse_intents_includes_poll_intents() {
        let intents = parse_gateway_intents("guild_message_polls,direct_message_polls").unwrap();
        assert!(intents.contains(Intents::GUILD_MESSAGE_POLLS));
        assert!(intents.contains(Intents::DIRECT_MESSAGE_POLLS));
    }

    #[test]
    fn parse_intents_is_case_insensitive_and_trimmed() {
        let intents = parse_gateway_intents("  GUILDS , Guild_Members , direct_message_reactions ").unwrap();
        assert!(intents.contains(Intents::GUILDS));
        assert!(intents.contains(Intents::GUILD_MEMBERS));
        assert!(intents.contains(Intents::DIRECT_MESSAGE_REACTIONS));
    }

    #[test]
    fn parse_intents_union_of_individual_keywords_is_all() {
        let intents = parse_gateway_intents(
            "guilds,guild_members,guild_moderation,guild_emojis_and_stickers,\
             guild_integrations,guild_webhooks,guild_invites,guild_voice_states,\
             guild_presences,guild_messages,guild_message_reactions,guild_message_typing,\
             direct_messages,direct_message_reactions,direct_message_typing,message_content,\
             guild_scheduled_events,auto_moderation_configuration,auto_moderation_execution,\
             guild_message_polls,direct_message_polls",
        )
        .unwrap();

        assert_eq!(intents, Intents::all());
    }

    #[test]
    fn discord_bot_token_roundtrips() {
        let token = DiscordBotToken::new("Bot token").unwrap();
        assert_eq!(token.as_str(), "Bot token");
    }

    #[test]
    fn discord_bot_token_debug_redacts() {
        let token = DiscordBotToken::new("Bot token").unwrap();
        assert_eq!(format!("{token:?}"), "DiscordBotToken(****)");
    }
