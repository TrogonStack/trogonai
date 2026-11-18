use std::sync::Arc;

use trogon_decider_runtime::StreamPosition;
use trogon_identity_types::SubjectPrincipal;

use crate::{SessionError, SessionKey};

pub struct SessionRuntime<S, A, C> {
    pub(crate) store: Arc<S>,
    pub(crate) admission: Arc<A>,
    pub(crate) codec: Arc<C>,
}

#[derive(Debug)]
pub struct RecoveredChild {
    pub child_key: SessionKey,
    pub creation_position: StreamPosition,
    pub inserted: bool,
}

impl<S, A, C> SessionRuntime<S, A, C> {
    pub fn new(store: Arc<S>, admission: Arc<A>, codec: Arc<C>) -> Self {
        Self { store, admission, codec }
    }

    pub async fn recover_child(
        &self,
        _caller: &SubjectPrincipal,
        _parent_key: &SessionKey,
        _operation_id: &str,
    ) -> Result<RecoveredChild, SessionError> {
        let _ = (&self.store, &self.admission, &self.codec);
        Err(SessionError::UnsupportedHistory)
    }
}
