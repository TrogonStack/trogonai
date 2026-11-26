use tokio_util::sync::CancellationToken;

pub(crate) enum RuntimePhase {
    Starting,
    Running(CancellationToken),
    Stopped,
}
