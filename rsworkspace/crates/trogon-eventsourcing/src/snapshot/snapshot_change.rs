use super::Snapshot;

#[derive(Debug, Clone, PartialEq)]
pub enum SnapshotChange<T> {
    Upsert {
        stream_id: String,
        snapshot: Box<Snapshot<T>>,
    },
    Delete {
        stream_id: String,
    },
}

impl<T> SnapshotChange<T> {
    pub fn upsert(stream_id: impl Into<String>, snapshot: Snapshot<T>) -> Self {
        Self::Upsert {
            stream_id: stream_id.into(),
            snapshot: Box::new(snapshot),
        }
    }

    pub fn delete(stream_id: impl Into<String>) -> Self {
        Self::Delete {
            stream_id: stream_id.into(),
        }
    }
}
