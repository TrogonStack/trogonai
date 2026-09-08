use async_nats::jetstream::kv;
use buffa::Message as _;

use super::*;

#[derive(Clone)]
struct ConcurrentDeletion;

impl JetStreamKvKeys for ConcurrentDeletion {
    type Keys = futures::stream::Iter<std::vec::IntoIter<Result<String, kv::WatcherError>>>;

    async fn keys(&self) -> Result<Self::Keys, kv::HistoryError> {
        Ok(futures::stream::iter(vec![
            Ok("removed".to_string()),
            Ok("present".to_string()),
        ]))
    }
}

impl JetStreamKvGet for ConcurrentDeletion {
    async fn get(&self, key: String) -> Result<Option<bytes::Bytes>, kv::EntryError> {
        if key == "removed" {
            return Ok(None);
        }
        let view = crate::projections_v1::ScheduleProjection {
            schedule_id: key,
            schedule: buffa::MessageField::some(crate::projections_v1::Schedule {
                kind: Some(
                    crate::projections_v1::schedule::Every {
                        every: buffa::MessageField::none(),
                    }
                    .into(),
                ),
            }),
            delivery: buffa::MessageField::some(crate::projections_v1::Delivery {
                kind: Some(
                    crate::projections_v1::delivery::NatsMessage {
                        subject: "agent.run".to_string(),
                        ttl: buffa::MessageField::none(),
                        source: buffa::MessageField::none(),
                    }
                    .into(),
                ),
            }),
            message: buffa::MessageField::some(crate::projections_v1::Message::default()),
            ..Default::default()
        };
        Ok(Some(view.encode_to_vec().into()))
    }
}

#[tokio::test]
async fn deletion_after_key_enumeration_does_not_hide_the_remaining_schedules() {
    let schedules = run(&ConcurrentDeletion, ListSchedules).await.unwrap();

    assert_eq!(schedules.len(), 1);
    assert_eq!(schedules[0].id, "present");
}
