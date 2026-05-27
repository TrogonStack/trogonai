pub mod ocsf;

use async_trait::async_trait;

use crate::error::TrafficViewError;
use crate::event::TrafficEvent;
use crate::siem::ocsf::OcsfExporter;

#[async_trait]
pub trait SiemExporter: Send + Sync {
    async fn export(&self, event: TrafficEvent) -> Result<(), TrafficViewError>;
}

pub struct OcsfSiemExporter;

#[async_trait]
impl SiemExporter for OcsfSiemExporter {
    async fn export(&self, event: TrafficEvent) -> Result<(), TrafficViewError> {
        let _record = ocsf::DefaultOcsfExporter.emit(&event);
        Ok(())
    }
}
