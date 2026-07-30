use async_nats::header::HeaderMap as NatsHeaderMap;
use http::HeaderMap as HttpHeaderMap;
use rmcp::transport::common::http_header::{
    HEADER_MCP_METHOD, HEADER_MCP_NAME, HEADER_MCP_PARAM_PREFIX, HEADER_MCP_PROTOCOL_VERSION,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpTransportHeaders {
    entries: Vec<McpTransportHeader>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct McpTransportHeader {
    name: String,
    value: String,
}

impl McpTransportHeaders {
    pub fn from_http(headers: &HttpHeaderMap) -> Self {
        let mut result = Self::default();
        for name in headers.keys() {
            let Some(name) = canonical_header_name(name.as_str()) else {
                continue;
            };
            for value in headers.get_all(name.as_str()) {
                if let Ok(value) = value.to_str() {
                    result.entries.push(McpTransportHeader {
                        name: name.clone(),
                        value: value.to_owned(),
                    });
                }
            }
        }
        result
    }

    pub fn from_nats(headers: &NatsHeaderMap) -> Self {
        let mut result = Self::default();
        for (name, values) in headers.iter() {
            let raw_name: &str = name.as_ref();
            let Some(name) = canonical_header_name(raw_name) else {
                continue;
            };
            for value in values {
                result.entries.push(McpTransportHeader {
                    name: name.clone(),
                    value: value.as_str().to_owned(),
                });
            }
        }
        result
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(name))
            .map(|entry| entry.value.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn extend_nats(&self, headers: &mut NatsHeaderMap) {
        for entry in &self.entries {
            headers.append(entry.name.as_str(), entry.value.as_str());
        }
    }
}

fn canonical_header_name(name: &str) -> Option<String> {
    for fixed in [HEADER_MCP_PROTOCOL_VERSION, HEADER_MCP_METHOD, HEADER_MCP_NAME] {
        if name.eq_ignore_ascii_case(fixed) {
            return Some(fixed.to_owned());
        }
    }

    name.get(..HEADER_MCP_PARAM_PREFIX.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(HEADER_MCP_PARAM_PREFIX))
        .and_then(|_| name.get(HEADER_MCP_PARAM_PREFIX.len()..))
        .filter(|suffix| !suffix.is_empty())
        .map(|suffix| format!("{HEADER_MCP_PARAM_PREFIX}{suffix}"))
}
