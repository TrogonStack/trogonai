use std::time::Duration;

use serde_json::json;
use trogon_sandbox_space_worker::{
    Attach, EventsUrlError, MaxLeases, MaxLeasesError, OperationState, SandboxState, SpaceConfig, SpaceConfigError,
    SpaceConfigParts, SpaceId, SpaceIdError, SpaceOrigin, SpaceOriginError, SpaceTemplate, SpaceTemplateError,
    SpaceToken, SpaceTokenError, SseError, SseParser,
};
use url::Url;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn origins_are_bare_and_secure() -> TestResult {
    assert!(SpaceOrigin::parse(Url::parse("https://space.example")?).is_ok());
    assert!(SpaceOrigin::parse(Url::parse("http://127.0.0.1:8080/")?).is_ok());
    assert_eq!(
        SpaceOrigin::parse(Url::parse("http://space.example")?).err(),
        Some(SpaceOriginError::InsecureRemote)
    );
    for text in [
        "https://u:p@space.example",
        "https://space.example/v1",
        "https://space.example?q=1",
        "https://space.example#f",
    ] {
        assert_eq!(
            SpaceOrigin::parse(Url::parse(text)?).err(),
            Some(SpaceOriginError::NotAnOrigin)
        );
    }
    let origin = SpaceOrigin::parse(Url::parse("https://space.example")?)?;
    assert!(origin.admits(&Url::parse("https://space.example/events")?));
    assert!(!origin.admits(&Url::parse("https://other.example/events")?));
    assert_eq!(origin.url().host_str(), Some("space.example"));
    Ok(())
}

#[test]
fn tokens_are_redacted() -> TestResult {
    assert_eq!(SpaceToken::parse("").err(), Some(SpaceTokenError::Empty));
    assert_eq!(
        SpaceToken::parse("bad\nvalue").err(),
        Some(SpaceTokenError::NotAHeaderValue)
    );
    let token = SpaceToken::parse("secret-value")?;
    assert_eq!(format!("{token:?}"), "SpaceToken(<redacted>)");
    let attach: Attach = serde_json::from_value(json!({"events_url": "/e", "token": "secret-value"}))?;
    let shown = format!("{attach:?}");
    assert!(shown.contains("<redacted>"));
    assert!(!shown.contains("secret-value"));
    Ok(())
}

#[test]
fn ids_are_safe_for_a_url_path() -> TestResult {
    assert!(SpaceId::parse("sb_1-a").is_ok());
    assert_eq!(
        SpaceId::parse("a".repeat(129)).err(),
        Some(SpaceIdError::TooLong { max: SpaceId::MAX_LEN })
    );
    assert_eq!(SpaceId::parse("a/b").err(), Some(SpaceIdError::Character));
    assert!(matches!(SpaceId::parse("").err(), Some(SpaceIdError::Sandbox(_))));
    assert!(serde_json::from_value::<SpaceId>(json!("a b")).is_err());
    Ok(())
}

#[test]
fn templates_are_one_word() -> TestResult {
    assert_eq!(SpaceTemplate::parse("").err(), Some(SpaceTemplateError::Empty));
    assert_eq!(SpaceTemplate::parse("a b").err(), Some(SpaceTemplateError::Character));
    assert_eq!(SpaceTemplate::parse("seed:latest")?.as_str(), "seed:latest");
    Ok(())
}

#[test]
fn capacity_is_bounded() -> TestResult {
    assert_eq!(
        MaxLeases::parse(0).err(),
        Some(MaxLeasesError {
            limit: MaxLeases::LIMIT
        })
    );
    assert_eq!(
        MaxLeases::parse(MaxLeases::LIMIT + 1).err(),
        Some(MaxLeasesError {
            limit: MaxLeases::LIMIT
        })
    );
    assert_eq!(MaxLeases::parse(3)?.get(), 3);
    Ok(())
}

fn parts(change: impl FnOnce(&mut SpaceConfigParts)) -> Result<SpaceConfig, SpaceConfigError> {
    let mut parts = SpaceConfigParts {
        template: SpaceTemplate::parse("seed:latest").map_err(|_| SpaceConfigError::ZeroDuration("template"))?,
        state_dir: std::env::temp_dir(),
        boot_timeout: Duration::from_secs(1),
        operation_timeout: Duration::from_secs(1),
        poll_interval: Duration::from_secs(1),
        max_leases: MaxLeases::parse(1).map_err(|_| SpaceConfigError::ZeroDuration("leases"))?,
        command_grace: Duration::from_secs(1),
        file_read_timeout: Duration::from_secs(1),
    };
    change(&mut parts);
    SpaceConfig::new(parts)
}

#[test]
fn settings_need_positive_durations() -> TestResult {
    assert!(parts(|_| {}).is_ok());
    assert_eq!(
        parts(|p| p.boot_timeout = Duration::ZERO).err(),
        Some(SpaceConfigError::ZeroDuration("boot timeout"))
    );
    assert_eq!(
        parts(|p| p.operation_timeout = Duration::ZERO).err(),
        Some(SpaceConfigError::ZeroDuration("operation timeout"))
    );
    assert_eq!(
        parts(|p| p.poll_interval = Duration::ZERO).err(),
        Some(SpaceConfigError::ZeroDuration("poll interval"))
    );
    assert_eq!(
        parts(|p| p.file_read_timeout = Duration::ZERO).err(),
        Some(SpaceConfigError::ZeroDuration("file read timeout"))
    );
    Ok(())
}

#[test]
fn states_keep_their_spelling() -> TestResult {
    for text in ["succeeded", "failed", "canceled", "expired", "running-ish"] {
        let state = OperationState::from(text.to_owned());
        assert_eq!(state.as_str(), text);
        assert_eq!(state.is_terminal(), text != "running-ish");
    }
    for text in [
        "running",
        "paused",
        "suspended",
        "deleting",
        "deleted",
        "starting",
        "failed",
    ] {
        let state: SandboxState = serde_json::from_value(json!(text))?;
        assert_eq!(state.as_str(), text);
    }
    assert!(SandboxState::Transitional("starting".into()).is_transitional());
    assert!(!SandboxState::Running.is_transitional());
    Ok(())
}

#[test]
fn error_messages_name_the_cause() -> TestResult {
    assert!(!format!("{}", SseError::FrameTooLarge).is_empty());
    assert!(!format!("{}", EventsUrlError::OffOrigin).is_empty());
    assert!(!format!("{}", SpaceConfigError::ZeroDuration("x")).is_empty());
    Ok(())
}

fn feed(parser: &mut SseParser, text: &str) -> Result<Vec<(Option<String>, serde_json::Value)>, SseError> {
    Ok(parser
        .feed(text.as_bytes())?
        .into_iter()
        .map(|frame| (frame.id, serde_json::Value::Object(frame.data)))
        .collect())
}

#[test]
fn parses_events_with_any_line_ending() -> TestResult {
    let mut parser = SseParser::default();
    let frames = feed(
        &mut parser,
        "id: 1\r\ndata: {\"a\":1}\r\n\r\n: comment\ndata:{\"b\":2}\rid: 2\r\r",
    )?;
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0], (Some("1".into()), json!({"a": 1})));
    assert_eq!(frames[1], (Some("2".into()), json!({"b": 2})));
    assert!(!parser.is_mid_frame());
    feed(&mut parser, "data: {")?;
    assert!(parser.is_mid_frame());
    Ok(())
}

#[test]
fn skips_fields_it_does_not_use() -> TestResult {
    let mut parser = SseParser::default();
    let frames = feed(&mut parser, "event: x\nretry: 5\ndata\ndata: {}\nid: a\0b\n\n")?;
    assert_eq!(frames, vec![(None, json!({}))]);
    Ok(())
}

#[test]
fn refuses_frames_that_are_not_objects_or_are_too_large() -> TestResult {
    let mut parser = SseParser::default();
    assert_eq!(feed(&mut parser, "data: nope\n\n").err(), Some(SseError::NotJson));
    let mut parser = SseParser::default();
    assert_eq!(feed(&mut parser, "data: [1]\n\n").err(), Some(SseError::NotObject));
    let mut parser = SseParser::default();
    let big = "x".repeat(2 * 1024 * 1024 + 1);
    assert_eq!(parser.feed(big.as_bytes()).err(), Some(SseError::FrameTooLarge));
    Ok(())
}
