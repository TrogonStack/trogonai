use super::*;
use trogon_std::env::InMemoryEnv;

/// Why a config did not load. `BridgeConfig` is not `Debug` (it holds a token),
/// so `expect_err` is out and the failure has to be taken by pattern.
fn rejection(env: &InMemoryEnv) -> BridgeConfigError {
    let Err(error) = BridgeConfig::from_env(env) else {
        panic!("this environment must not configure the bridge");
    };
    error
}

/// A token is the one required variable, so every way of not supplying one has
/// to fail the same way. Compose renders an unset variable as the empty string,
/// which is why blank is not merely tolerated-but-odd: it is the common case.
#[test]
fn a_blank_bot_token_fails_like_an_unset_one() {
    for token in ["", "   ", "\n"] {
        assert!(matches!(BotToken::new(token), Err(BlankBotTokenError)), "{token:?}");

        let env = InMemoryEnv::new();
        env.set("TELEGRAM_BOT_TOKEN", token);
        assert!(
            matches!(rejection(&env), BridgeConfigError::BotToken(_)),
            "blank token {token:?} must be refused as a token, not as something else"
        );
    }

    assert!(matches!(rejection(&InMemoryEnv::new()), BridgeConfigError::BotToken(_)));
}

/// The bridge resolves claims from exactly the bucket the gateway publishes
/// them to. The gateway provisions `DEFAULT_CLAIM_BUCKET` unconditionally and
/// stamps that name into every claim header, so this is not a default the
/// environment may override: a different value here resolves nothing. Setting
/// the variable that used to steer it must therefore change nothing.
#[test]
fn the_claim_bucket_is_the_gateways_bucket_and_the_environment_cannot_move_it() {
    let env = InMemoryEnv::new();
    env.set("TELEGRAM_BOT_TOKEN", "secret-token");
    env.set("TROGON_CLAIM_BUCKET", "somewhere-the-gateway-never-writes");

    let config = BridgeConfig::from_env(&env).expect("config");
    assert_eq!(config.claim_bucket, ClaimBucket::default());
}

/// A token read from a file or a heredoc arrives with a trailing newline, which
/// Telegram rejects without saying which byte was wrong.
#[test]
fn a_bot_token_is_trimmed() {
    let env = InMemoryEnv::new();
    env.set("TELEGRAM_BOT_TOKEN", "  secret-token\n");
    let config = BridgeConfig::from_env(&env).expect("config");
    assert_eq!(config.bot_token.as_str(), "secret-token");
}

#[test]
fn a_bot_token_does_not_print_itself() {
    let token = BotToken::new("secret-token").expect("token");
    let rendered = format!("{token:?}");
    assert!(
        !rendered.contains("secret-token"),
        "token leaked into Debug: {rendered}"
    );
}

/// Every optional variable has a default, and a deployment that renders an
/// unset variable as blank must land on that default rather than on an empty
/// bucket prefix or stream name.
#[test]
fn blank_optional_variables_fall_back_to_their_defaults() {
    let env = InMemoryEnv::new();
    env.set("TELEGRAM_BOT_TOKEN", "secret-token");
    for key in [
        "CHANNEL_PREFIX",
        "TELEGRAM_INBOUND_STREAM",
        "TELEGRAM_BOT_ACCOUNT",
        "CHANNEL_AGENT_ID",
        "CHANNEL_AGENT_CWD",
        "CHANNEL_SEED_TELEGRAM_USERS",
    ] {
        env.set(key, "");
    }

    let config = BridgeConfig::from_env(&env).expect("config");
    assert_eq!(config.channel_prefix, "prod");
    assert_eq!(config.inbound_stream, "TELEGRAM");
    assert_eq!(config.bot_account, "bot");
    assert_eq!(config.agent_id, "default");
    assert_eq!(config.agent_cwd, std::env::temp_dir());
    assert!(config.seed_users.is_empty());
}

/// The trigger list is the exception to blank-means-absent: an empty list is how
/// a deployment says "recognize no commands and forward everything", so a blank
/// value must not be quietly replaced by the defaults.
#[test]
fn a_blank_trigger_list_means_no_triggers_rather_than_the_defaults() {
    let env = InMemoryEnv::new();
    env.set("TELEGRAM_BOT_TOKEN", "secret-token");
    env.set("CHANNEL_NEW_SESSION_TRIGGERS", "");
    let config = BridgeConfig::from_env(&env).expect("config");
    assert_eq!(config.command_triggers.parse("/new", "bot").command, None);

    let env = InMemoryEnv::new();
    env.set("TELEGRAM_BOT_TOKEN", "secret-token");
    let config = BridgeConfig::from_env(&env).expect("config");
    assert_eq!(
        config.command_triggers.parse("/new", "bot").command,
        Some(trogon_channel::Command::NewSession)
    );
}

/// A seeded user id becomes a Telegram chat id, so a typo has to stop the
/// bridge at boot rather than silently seed a shorter list: the operator who
/// wrote it would otherwise find out only when that person is refused, and the
/// message has to name the entry so they know which one.
#[test]
fn a_seed_list_with_an_unparseable_id_fails_and_names_it() {
    let env = InMemoryEnv::new();
    env.set("TELEGRAM_BOT_TOKEN", "secret-token");
    env.set("CHANNEL_SEED_TELEGRAM_USERS", "42, not-an-id ,43");

    let error = rejection(&env);
    let BridgeConfigError::SeedUser { entry, .. } = &error else {
        panic!("an unparseable seed id must be refused as one: {error}");
    };
    assert_eq!(entry, "not-an-id");
    assert!(
        error.to_string().contains("not-an-id"),
        "the operator has to be told which entry to go and fix: {error}"
    );
}

/// The trigger list and the ACP prefix are the other two values a deployment can
/// get wrong, and each has to be refused as itself: an operator reading a boot
/// failure is being told which variable to go and edit.
#[test]
fn each_unusable_value_is_refused_as_the_variable_it_came_from() {
    let env = InMemoryEnv::new();
    env.set("TELEGRAM_BOT_TOKEN", "secret-token");
    env.set("CHANNEL_NEW_SESSION_TRIGGERS", "/new session");
    assert!(matches!(
        rejection(&env),
        BridgeConfigError::CommandTriggers(CommandTriggerError::MultipleTokens)
    ));

    let env = InMemoryEnv::new();
    env.set("TELEGRAM_BOT_TOKEN", "secret-token");
    env.set(acp_nats::ENV_ACP_PREFIX, "not a prefix");
    assert!(matches!(
        rejection(&env),
        BridgeConfigError::AcpPrefix(AcpPrefixError::InvalidCharacter(' '))
    ));
}

#[test]
fn set_variables_are_read_and_trimmed() {
    let env = InMemoryEnv::new();
    env.set("TELEGRAM_BOT_TOKEN", "secret-token");
    env.set("CHANNEL_PREFIX", " staging ");
    env.set("TELEGRAM_INBOUND_STREAM", "TG");
    env.set("TELEGRAM_BOT_ACCOUNT", "mybot");
    env.set("CHANNEL_AGENT_ID", "coder");
    env.set("CHANNEL_AGENT_CWD", "/workspace");
    env.set("CHANNEL_SEED_TELEGRAM_USERS", "42, 43 ,,");

    let config = BridgeConfig::from_env(&env).expect("config");
    assert_eq!(config.channel_prefix, "staging");
    assert_eq!(config.inbound_stream, "TG");
    assert_eq!(config.bot_account, "mybot");
    assert_eq!(config.agent_id, "coder");
    assert_eq!(config.agent_cwd, PathBuf::from("/workspace"));
    assert_eq!(config.seed_users, vec![42, 43]);
}
