use agent_client_protocol::{
    Implementation, InitializeRequest, InitializeResponse, ProtocolVersion,
};

pub struct AcpInitializeRequestBuilder {
    client_name: String,
}

impl AcpInitializeRequestBuilder {
    pub fn new() -> Self {
        Self {
            client_name: "zed".to_string(),
        }
    }

    pub fn with_client(mut self, name: &str) -> Self {
        self.client_name = name.to_string();
        self
    }

    pub fn build_payload(&self) -> bytes::Bytes {
        let request = InitializeRequest::new(ProtocolVersion::LATEST)
            .client_info(Implementation::new(&self.client_name, "1.0.0"));
        serde_json::to_vec(&request)
            .expect("InitializeRequest serialization should not fail")
            .into()
    }
}

pub struct AcpInitializeResponseBuilder {
    result: std::result::Result<InitializeResponse, (i32, String)>,
}

impl AcpInitializeResponseBuilder {
    pub fn success() -> Self {
        Self {
            result: Ok(InitializeResponse::new(ProtocolVersion::LATEST)),
        }
    }

    pub fn error(code: i32, message: &str) -> Self {
        Self {
            result: Err((code, message.to_string())),
        }
    }

    pub fn build_payload(&self) -> bytes::Bytes {
        match &self.result {
            Ok(response) => serde_json::to_vec(response)
                .expect("InitializeResponse serialization should not fail")
                .into(),
            Err((code, message)) => {
                let error = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": code,
                        "message": message
                    }
                });
                serde_json::to_vec(&error)
                    .expect("JSON-RPC error serialization should not fail")
                    .into()
            }
        }
    }
}
