#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

use std::net::SocketAddr;
use std::sync::Arc;

use ard_catalog::{
    CatalogEntryWire, CatalogHostWire, CatalogManifest, CatalogManifestWire, CatalogManifestWireError, SPEC_VERSION,
};
use ard_registry::{Registry, RegistryConfig, SourceUrl, router};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let first = args.next();
    if first.as_deref() == Some("--write-manifest") {
        let path = args.next().ok_or("--write-manifest requires a path")?;
        let json = serde_json::to_string_pretty(&demo_manifest()?.into_wire())?;
        std::fs::write(path, json)?;
        return Ok(());
    }

    let listen = first.unwrap_or_else(|| "127.0.0.1:0".to_owned());
    let address: SocketAddr = listen.parse()?;
    let listener = TcpListener::bind(address).await?;
    println!("{}", listener.local_addr()?);

    let registry = Arc::new(Registry::new(RegistryConfig::new(
        SourceUrl::parse("http://127.0.0.1")?,
        demo_manifest()?,
        vec![],
    )));
    axum::serve(listener, router(registry)).await?;
    Ok(())
}

fn demo_manifest() -> Result<CatalogManifest, CatalogManifestWireError> {
    CatalogManifestWire {
        spec_version: SPEC_VERSION.to_owned(),
        host: Some(CatalogHostWire(serde_json::json!({
            "displayName": "Trogon ARD Demo Registry"
        }))),
        entries: vec![
            CatalogEntryWire {
                identifier: "urn:air:trogon.ai:mcp:weather".to_owned(),
                display_name: "Weather Forecast Tools".to_owned(),
                media_type: "application/mcp-server-card+json".to_owned(),
                url: Some("https://example.com/mcp/weather-card.json".to_owned()),
                data: None,
                description: Some("Weather forecast tools for ARD conformance probes".to_owned()),
                representative_queries: Some(vec![
                    "weather forecast tools".to_owned(),
                    "current weather by city".to_owned(),
                ]),
                tags: Some(vec!["weather".to_owned(), "tools".to_owned()]),
                capabilities: Some(vec!["forecast".to_owned(), "tools".to_owned()]),
                version: Some("0.1.0".to_owned()),
                updated_at: None,
                metadata: None,
                trust_manifest: None,
            },
            CatalogEntryWire {
                identifier: "urn:air:trogon.ai:agent:assistant".to_owned(),
                display_name: "Assistant Agent".to_owned(),
                media_type: "application/a2a-agent-card+json".to_owned(),
                url: Some("https://example.com/a2a/assistant-card.json".to_owned()),
                data: None,
                description: Some("General assistant agent".to_owned()),
                representative_queries: Some(vec!["answer questions".to_owned(), "help with tasks".to_owned()]),
                tags: Some(vec!["assistant".to_owned()]),
                capabilities: Some(vec!["chat".to_owned()]),
                version: Some("0.1.0".to_owned()),
                updated_at: None,
                metadata: None,
                trust_manifest: None,
            },
        ],
    }
    .try_into()
}

#[cfg(test)]
mod tests;
