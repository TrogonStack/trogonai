use zeroize::Zeroize;

#[derive(PartialEq, prost::Message)]
#[prost(skip_debug)]
pub(crate) struct WorkloadIdentityWire {
    #[prost(string, tag = "1")]
    pub spiffe_id: String,
    #[prost(bytes = "vec", tag = "2")]
    pub x509_svid: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub x509_svid_key: Vec<u8>,
    #[prost(bytes = "vec", tag = "4")]
    pub bundle: Vec<u8>,
    #[prost(string, tag = "5")]
    pub hint: String,
}

impl std::fmt::Debug for WorkloadIdentityWire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkloadIdentityWire { <redacted> }")
    }
}

impl Drop for WorkloadIdentityWire {
    fn drop(&mut self) {
        self.x509_svid_key.zeroize();
    }
}

#[cfg(test)]
mod tests;
