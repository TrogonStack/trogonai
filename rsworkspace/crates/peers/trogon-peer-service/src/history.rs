use crate::constants::MAX_HISTORY_EVENTS;
use crate::history_error::HistoryError;
use trogon_decider_runtime::{ReadFrom, ReadStreamRequest, StreamPosition, StreamRead, read_stream_for_execution};
use trogon_peer::{PeerId, commands::state};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1::PeerEvent};

pub(crate) async fn load<Store: StreamRead<PeerId>>(
    store: &Store,
    peer: &PeerId,
) -> Result<PeerState, HistoryError<Store::Error>> {
    let (state, _) = load_with_position(store, peer).await?;
    Ok(state)
}

pub(crate) async fn load_with_position<Store: StreamRead<PeerId>>(
    store: &Store,
    peer: &PeerId,
) -> Result<(PeerState, Option<StreamPosition>), HistoryError<Store::Error>> {
    let read = read_stream_for_execution(
        store,
        ReadStreamRequest {
            stream_id: peer,
            from: ReadFrom::Beginning,
        },
        Some(MAX_HISTORY_EVENTS + 1),
    )
    .await
    .map_err(HistoryError::Read)?;
    if read.events.len() as u64 > MAX_HISTORY_EVENTS {
        return Err(HistoryError::Limit);
    }
    let mut result = state::initial_state();
    for event in read.events {
        let decoded = event
            .decode::<PeerEvent>()
            .map_err(HistoryError::Decode)?
            .into_decoded()
            .ok_or(HistoryError::UnknownEvent)?;
        result = state::evolve(result, &decoded).map_err(HistoryError::Evolve)?;
    }
    Ok((result, read.current_position))
}
