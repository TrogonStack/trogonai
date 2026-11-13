use trogon_claude_agent_sdk::{
    InMemorySessionStore, SessionEntry, SessionId, SessionKey, SessionProjectKey, SessionStore,
};

#[derive(serde::Serialize)]
struct TranscriptEntry<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    uuid: &'a str,
    message: TranscriptMessage<'a>,
}

#[derive(serde::Serialize)]
struct TranscriptMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        debug_remnants,
        reason = "This example prints its defined result or connection status to the caller."
    )
)]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemorySessionStore::new();
    let key = SessionKey::new(
        SessionProjectKey::new("example-project")?,
        SessionId::from("123e4567-e89b-42d3-a456-426614174000"),
    );
    let entry = SessionEntry::try_from(serde_json::to_value(TranscriptEntry {
        kind: "user",
        uuid: "example-message",
        message: TranscriptMessage {
            role: "user",
            content: "hello",
        },
    })?)?;
    store.append(&key, &[entry]).await?;
    let entries = store.load(&key).await?.unwrap_or_default();
    println!("Stored transcript entries: {}", entries.len());
    Ok(())
}
