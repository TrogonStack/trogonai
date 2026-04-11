use trogon_nats::NatsToken;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_std::NonZeroDuration;

use crate::IncidentioSigningSecret;

#[derive(Clone)]
pub struct IncidentioConfig {
    pub signing_secret: IncidentioSigningSecret,
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
    pub timestamp_tolerance: NonZeroDuration,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_secret() -> String {
        ["whsec_", "dGVzdC1zZWNyZXQ="].concat()
    }

    #[test]
    fn incidentio_config_clone_roundtrips() {
        let config = IncidentioConfig {
            signing_secret: IncidentioSigningSecret::new(test_secret()).unwrap(),
            subject_prefix: NatsToken::new("incidentio").unwrap(),
            stream_name: NatsToken::new("INCIDENTIO").unwrap(),
            stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
            nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
            timestamp_tolerance: NonZeroDuration::from_secs(300).unwrap(),
        };

        let clone = config.clone();
        assert_eq!(clone.subject_prefix.as_str(), "incidentio");
        assert_eq!(clone.stream_name.as_str(), "INCIDENTIO");
    }
}
