use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::{DottedNatsToken, NatsToken};
use trogon_std::{ByteSize, HttpBodySizeMax, NonZeroDuration, SecretString};

use super::meta_object_id::MetaObjectId;

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::b(3_000_000)).unwrap();

#[derive(Clone)]
pub struct WhatsAppWebhookConfig {
    pub verify_token: SecretString,
    pub app_secret: SecretString,
    pub waba_id: MetaObjectId,
    pub phone_number_id: MetaObjectId,
    pub max_body_bytes: HttpBodySizeMax,
}

#[derive(Clone)]
pub struct WhatsAppConfig {
    pub webhook: WhatsAppWebhookConfig,
    pub subject_prefix: DottedNatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
}
