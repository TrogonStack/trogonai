use async_nats::jetstream::stream::Config;

use crate::acp_prefix::AcpPrefix;
use crate::nats::AcpStream;

pub fn notifications_stream_name(prefix: &AcpPrefix) -> String {
    AcpStream::Notifications.stream_name(prefix)
}

pub fn responses_stream_name(prefix: &AcpPrefix) -> String {
    AcpStream::Responses.stream_name(prefix)
}

pub fn commands_stream_name(prefix: &AcpPrefix) -> String {
    AcpStream::Commands.stream_name(prefix)
}

pub fn global_stream_name(prefix: &AcpPrefix) -> String {
    AcpStream::Global.stream_name(prefix)
}

pub fn global_ext_stream_name(prefix: &AcpPrefix) -> String {
    AcpStream::GlobalExt.stream_name(prefix)
}

pub fn all_configs(prefix: &AcpPrefix) -> [Config; 6] {
    AcpStream::all_configs(prefix)
}

#[cfg(test)]
mod tests;
