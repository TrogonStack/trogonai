use super::GoogleCredentials;

pub const FAKE_SERVICE_ACCOUNT: &str = r#"{"type":"service_account","project_id":"fake-project","private_key_id":"fake-key-id","private_key":"-----BEGIN PRIVATE KEY-----\nFAKEFAKEFAKE\n-----END PRIVATE KEY-----\n","client_email":"fake@fake-project.iam.gserviceaccount.com"}"#;

pub const FAKE_EXTERNAL_ACCOUNT: &str = r#"{"type":"external_account","audience":"//iam.googleapis.com/projects/1/locations/global/workloadIdentityPools/fake/providers/fake","subject_token_type":"urn:ietf:params:oauth:token-type:jwt","token_url":"https://sts.googleapis.com/v1/token","credential_source":{"file":"/var/run/fake/token"}}"#;

pub fn fake_credentials() -> GoogleCredentials {
    GoogleCredentials::parse(FAKE_SERVICE_ACCOUNT).expect("fixture is a valid service account document")
}
