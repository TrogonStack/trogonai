use crate::peer_application_error::PeerApplicationError;
use trogon_decider_runtime::{StreamAppend, StreamRead};
use trogon_peer::PeerId;
pub type ApplicationResult<T, S> =
    Result<T, PeerApplicationError<<S as StreamRead<PeerId>>::Error, <S as StreamAppend<PeerId>>::Error>>;
pub struct PeerApplication<S, P, C, Content, Cache, Ids, Catalog> {
    pub(crate) store: S,
    pub(crate) policy: P,
    pub(crate) clock: C,
    pub(crate) content: Content,
    pub(crate) cache: Cache,
    pub(crate) ids: Ids,
    pub(crate) catalog: Catalog,
}
impl<S, P, C, Content, Cache, Ids, Catalog> PeerApplication<S, P, C, Content, Cache, Ids, Catalog> {
    pub fn new(store: S, policy: P, clock: C, content: Content, cache: Cache, ids: Ids, catalog: Catalog) -> Self {
        Self {
            store,
            policy,
            clock,
            content,
            cache,
            ids,
            catalog,
        }
    }
}
