use crate::constants::MAX_HISTORY_EVENTS;
use crate::history_error::HistoryError;
use trogon_decider_runtime::{ReadFrom, ReadStreamRequest, StreamPosition, StreamRead, read_stream_for_execution};
use trogon_peer::{PeerHistory, PeerId};
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
    let (history, position) = load_peer_history_with_position(store, peer).await?;
    Ok((history.into_state(), position))
}

pub(crate) async fn load_peer_history_with_position<Store: StreamRead<PeerId>>(
    store: &Store,
    peer: &PeerId,
) -> Result<(PeerHistory, Option<StreamPosition>), HistoryError<Store::Error>> {
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
    let validated_position = read.events.last().map(|event| event.stream_position);
    if validated_position != read.current_position {
        return Err(HistoryError::Incomplete);
    }
    let mut result = PeerHistory::initial();
    for event in read.events {
        let decoded = event
            .decode::<PeerEvent>()
            .map_err(HistoryError::Decode)?
            .into_decoded()
            .ok_or(HistoryError::UnknownEvent)?;
        result = result.evolve(&decoded).map_err(HistoryError::Evolve)?;
    }
    Ok((result, validated_position))
}
