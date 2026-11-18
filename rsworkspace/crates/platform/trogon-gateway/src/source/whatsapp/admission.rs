use std::borrow::Cow;

use serde::Deserialize;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};

use super::constants::MAX_BATCH_UPDATES;
use super::meta_object_id::MetaObjectId;

#[derive(Deserialize)]
struct BatchInput<'a> {
    #[serde(rename = "object")]
    _object: ObjectInput,
    #[serde(borrow)]
    entry: Vec<EntryInput<'a>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ObjectInput {
    WhatsappBusinessAccount,
}

#[derive(Deserialize)]
struct EntryInput<'a> {
    #[serde(borrow)]
    id: Cow<'a, str>,
    #[serde(borrow)]
    changes: Vec<ChangeInput<'a>>,
}

#[derive(Deserialize)]
struct ChangeInput<'a> {
    field: FieldInput,
    #[serde(borrow)]
    value: &'a RawValue,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FieldInput {
    Messages,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct MessagesInput<'a> {
    #[serde(borrow)]
    metadata: Option<MetadataInput<'a>>,
    #[serde(borrow)]
    messages: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct MetadataInput<'a> {
    #[serde(borrow)]
    phone_number_id: Cow<'a, str>,
}

#[derive(Deserialize)]
struct MessageInput<'a> {
    #[serde(borrow)]
    id: Cow<'a, str>,
    #[serde(rename = "type", borrow)]
    kind: Cow<'a, str>,
}

pub(super) struct InboundEvent<'a> {
    waba_id: &'a MetaObjectId,
    phone_number_id: &'a MetaObjectId,
    message: &'a RawValue,
    message_id: Cow<'a, str>,
}

impl InboundEvent<'_> {
    pub(super) fn payload(&self) -> String {
        // The IDs are decimal strings and the message already passed JSON validation.
        format!(
            r#"{{"waba_id":"{}","phone_number_id":"{}","message":{}}}"#,
            self.waba_id.as_str(),
            self.phone_number_id.as_str(),
            self.message.get()
        )
    }

    pub(super) fn dedup_id(&self) -> String {
        let mut hash = Sha256::new();
        // Decimal account and phone IDs cannot contain the separator.
        hash.update(self.waba_id.as_str());
        hash.update(b":");
        hash.update(self.phone_number_id.as_str());
        hash.update(b":");
        hash.update(self.message_id.as_bytes());
        hex::encode(hash.finalize())
    }
}

#[derive(Debug, thiserror::Error)]
pub(super) enum AdmissionError {
    #[error("invalid WhatsApp payload")]
    Json(#[from] serde_json::Error),
    #[error("inbound WhatsApp message is missing destination metadata")]
    MissingDestination,
    #[error("inbound WhatsApp message needs a nonempty id and type")]
    MissingMessageIdentity,
    #[error("WhatsApp batch exceeds the update limit")]
    TooManyUpdates,
    #[error("WhatsApp messages are outside the configured binding")]
    ForeignBinding,
}

pub(super) fn admit<'a>(
    body: &'a [u8],
    waba_id: &'a MetaObjectId,
    phone_number_id: &'a MetaObjectId,
) -> Result<Vec<InboundEvent<'a>>, AdmissionError> {
    let batch: BatchInput<'_> = serde_json::from_slice(body)?;
    if batch.entry.iter().map(|entry| entry.changes.len()).sum::<usize>() > MAX_BATCH_UPDATES {
        return Err(AdmissionError::TooManyUpdates);
    }

    let mut events = Vec::new();
    let mut foreign_change = false;
    for entry in batch.entry {
        for change in entry.changes {
            if change.field != FieldInput::Messages {
                continue;
            }
            if entry.id != waba_id.as_str() {
                foreign_change = true;
                continue;
            }
            let input: MessagesInput<'_> = serde_json::from_str(change.value.get())?;
            let Some(messages) = input.messages else {
                continue;
            };
            let metadata = input.metadata.ok_or(AdmissionError::MissingDestination)?;
            if metadata.phone_number_id != phone_number_id.as_str() {
                foreign_change = true;
                continue;
            }

            let messages: Vec<&RawValue> = serde_json::from_str(messages.get())?;
            if events.len() + messages.len() > MAX_BATCH_UPDATES {
                return Err(AdmissionError::TooManyUpdates);
            }
            for message in messages {
                let input: MessageInput<'_> = serde_json::from_str(message.get())?;
                if input.id.is_empty() || input.kind.is_empty() {
                    return Err(AdmissionError::MissingMessageIdentity);
                }
                events.push(InboundEvent {
                    waba_id,
                    phone_number_id,
                    message,
                    message_id: input.id,
                });
            }
        }
    }
    if events.is_empty() && foreign_change {
        return Err(AdmissionError::ForeignBinding);
    }
    Ok(events)
}
