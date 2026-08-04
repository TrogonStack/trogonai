use super::*;
use trogon_std::env::InMemoryEnv;

/// Why a config refused to load. Spelled out instead of `expect_err` because a
/// `BridgeConfig` is not `Debug`, which is the point: it holds a token.
fn rejected(env: &InMemoryEnv) -> String {
    match BridgeConfig::from_env(env) {
        Ok(_) => panic!("config must not load"),
        Err(error) => error.to_string(),
    }
}

/// A token is the one required variable, so every way of not supplying one has
/// to fail the same way. Compose renders an unset variable as the empty string,
/// which is why blank is not merely tolerated-but-odd: it is the common case.
#[test]
fn a_blank_bot_token_fails_like_an_unset_one() {
    for token in ["", "   ", "\n"] {
        let env = InMemoryEnv::new();
        env.set("TELEGRAM_BOT_TOKEN", token);
        let error = rejected(&env);
        assert!(
            error.contains("TELEGRAM_BOT_TOKEN not set"),
            "unexpected error for {token:?}: {error}"
        );
    }

    let error = rejected(&InMemoryEnv::new());
    assert!(
        error.contains("TELEGRAM_BOT_TOKEN not set"),
        "unexpected error: {error}"
    );
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
        "TROGON_CLAIM_BUCKET",
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
    assert_eq!(config.claim_bucket, trogon_nats::jetstream::DEFAULT_CLAIM_BUCKET);
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
    assert_eq!(config.command_triggers.parse("/new").command, None);

    let env = InMemoryEnv::new();
    env.set("TELEGRAM_BOT_TOKEN", "secret-token");
    let config = BridgeConfig::from_env(&env).expect("config");
    assert_eq!(
        config.command_triggers.parse("/new").command,
        Some(trogon_channel::Command::NewSession)
    );
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
