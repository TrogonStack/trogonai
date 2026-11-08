use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use trogon_nats::NatsToken;

use crate::chain_head::ChainHead;
use crate::chain_key::ChainKey;

/// What a checkpoint task needs from a writer: which chain it is, the component
/// its checkpoints are filed under, its acknowledged head as it moves, and when
/// it has stopped moving.
///
/// It holds no part of the writer, so following the head never takes the chain
/// gate, and a writer dropped without a drain task closes `heads`.
#[derive(Debug, Clone)]
pub(crate) struct HeadWatch {
    pub(crate) key: ChainKey,
    pub(crate) component: NatsToken,
    pub(crate) heads: watch::Receiver<Option<ChainHead>>,
    pub(crate) stopped: CancellationToken,
}

impl HeadWatch {
    pub(crate) fn new(
        key: ChainKey,
        component: NatsToken,
        heads: watch::Receiver<Option<ChainHead>>,
        stopped: CancellationToken,
    ) -> Self {
        Self {
            key,
            component,
            heads,
            stopped,
        }
    }
}
