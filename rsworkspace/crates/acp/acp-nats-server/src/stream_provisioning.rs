use acp_nats::{AcpPrefix, nats::AcpStream};
use async_nats::jetstream::{Context, ErrorCode, context::GetStreamErrorKind};
use trogon_nats::jetstream::NatsJetStreamClient;

use crate::provision_command_error::ProvisionCommandError;

pub async fn run(context: &Context, prefix: &AcpPrefix) -> Result<(), ProvisionCommandError> {
    for stream in AcpStream::ALL {
        match context.get_stream(stream.stream_name(prefix)).await {
            Ok(current) => {
                let config = &current.cached_info().config;
                if config.sealed {
                    return Err(ProvisionCommandError::Sealed {
                        prefix: prefix.clone(),
                        stream,
                    });
                }
                if config.mirror.is_some() {
                    return Err(ProvisionCommandError::Mirrored {
                        prefix: prefix.clone(),
                        stream,
                    });
                }
            }
            Err(source) => match source.kind() {
                GetStreamErrorKind::JetStream(error) if error.error_code() == ErrorCode::STREAM_NOT_FOUND => {}
                _ => {
                    return Err(ProvisionCommandError::Inspect {
                        prefix: prefix.clone(),
                        stream,
                        source,
                    });
                }
            },
        }
    }

    let client = NatsJetStreamClient::new(context.clone());
    acp_nats::jetstream::provision::provision_streams(&client, prefix).await?;
    Ok(())
}
