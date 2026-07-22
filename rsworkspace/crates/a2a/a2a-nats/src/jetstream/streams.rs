use async_nats::jetstream::stream::Config;

use crate::a2a_prefix::A2aPrefix;
use crate::nats::subjects::A2aStream;

use super::stream_options::StreamProvisionOptions;

pub fn events_stream_name(prefix: &A2aPrefix) -> String {
    A2aStream::Events.stream_name(prefix)
}

pub fn all_configs(prefix: &A2aPrefix) -> [Config; 2] {
    A2aStream::all_configs(prefix)
}

pub fn all_configs_with_options(prefix: &A2aPrefix, options: &StreamProvisionOptions) -> [Config; 2] {
    super::stream_options::all_configs_with_options(prefix, options)
}

#[cfg(test)]
mod tests;
