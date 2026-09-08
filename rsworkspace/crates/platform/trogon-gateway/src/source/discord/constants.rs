use twilight_model::gateway::Intents;

pub const NATS_HEADER_EVENT_NAME: &str = "X-Discord-Event-Name";
pub const NATS_HEADER_GUILD_ID: &str = "X-Discord-Guild-Id";

pub const PRIVILEGED_INTENTS: Intents = Intents::from_bits_truncate(
    Intents::GUILD_MEMBERS.bits() | Intents::GUILD_PRESENCES.bits() | Intents::MESSAGE_CONTENT.bits(),
);

pub const GATEWAY_OP_DISPATCH: u8 = 0;
