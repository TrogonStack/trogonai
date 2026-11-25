use crate::{Error, ObjectId};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConnectionId(ObjectId);

impl ConnectionId {
    pub fn new(value: String) -> Result<Self, Error> {
        if value.len() > crate::constants::MAX_CONNECTION_ID_BYTES {
            return Err(Error::ObjectId);
        }
        ObjectId::new(value).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn into_object_id(self) -> ObjectId {
        self.0
    }
}
