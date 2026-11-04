use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub(crate) fn install() {
    let output = tracing_subscriber::fmt::layer()
        .without_time()
        .with_level(false)
        .with_target(false)
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .with_filter(filter_fn(|metadata| metadata.target() == "peer_connector"));
    let _installed = tracing_subscriber::registry().with(output).try_init();
}

pub(crate) fn invalid_settings() {
    tracing::warn!(target: "peer_connector", "Peer connector settings are invalid or credentials are missing.");
}

pub(crate) fn unconfirmed_shutdown() {
    tracing::warn!(target: "peer_connector", "Peer connector stopped without confirming clean shutdown; remote outcomes remain uncertain.");
}
