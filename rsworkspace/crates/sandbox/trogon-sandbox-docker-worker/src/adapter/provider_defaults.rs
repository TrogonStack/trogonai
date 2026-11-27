use super::*;

pub fn build_oci_client(insecure_registries: Vec<RegistryHost>) -> OciClient {
    OciClient::new(OciClientConfig {
        insecure_registries,
        ..Default::default()
    })
}

pub fn default_warm_pool_config() -> WarmPoolConfig {
    WarmPoolConfig::default()
}
