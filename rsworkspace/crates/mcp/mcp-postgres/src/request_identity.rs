use crate::ObjectId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestIdentity {
    user: ObjectId,
    client: ObjectId,
    delegation: Vec<ObjectId>,
}

impl RequestIdentity {
    pub fn new(user: ObjectId, client: ObjectId, delegation: Vec<ObjectId>) -> Self {
        Self {
            user,
            client,
            delegation,
        }
    }

    pub fn user(&self) -> &ObjectId {
        &self.user
    }

    pub fn client(&self) -> &ObjectId {
        &self.client
    }

    pub fn delegation(&self) -> &[ObjectId] {
        &self.delegation
    }
}
