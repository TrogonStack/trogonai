use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct CreateMachineBody<'a> {
    pub name: &'a str,
    pub region: &'a str,
    pub skip_launch: bool,
    pub config: MachineConfigBody<'a>,
}

#[derive(Serialize)]
pub(crate) struct MachineConfigBody<'a> {
    pub image: &'a str,
    pub init: InitBody,
    pub guest: GuestBody,
    pub restart: RestartBody,
    pub auto_destroy: bool,
    pub metadata: BTreeMap<&'a str, &'a str>,
    pub services: Vec<ServiceBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mounts: Option<[MountBody<'a>; 1]>,
}

#[derive(Serialize)]
pub(crate) struct InitBody {
    pub exec: [&'static str; 2],
}

#[derive(Serialize)]
pub(crate) struct GuestBody {
    pub cpus: u32,
    pub cpu_kind: &'static str,
    pub memory_mb: u32,
}

#[derive(Serialize)]
pub(crate) struct RestartBody {
    pub policy: &'static str,
}

#[derive(Serialize)]
pub(crate) struct ServiceBody {
    pub protocol: &'static str,
    pub internal_port: u16,
    pub autostop: &'static str,
    pub autostart: bool,
    pub ports: [PortBody; 1],
}

#[derive(Serialize)]
pub(crate) struct PortBody {
    pub port: u16,
    pub handlers: &'static [&'static str],
}

#[derive(Serialize)]
pub(crate) struct MountBody<'a> {
    pub volume: &'a str,
    pub path: &'a str,
}

#[derive(Serialize)]
pub(crate) struct CreateVolumeBody<'a> {
    pub name: &'a str,
    pub region: &'a str,
    pub size_gb: u32,
    pub encrypted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<&'a str>,
}

#[derive(Serialize)]
pub(crate) struct DeniedBody {
    pub outcome: &'static str,
    pub kind: &'static str,
    pub reason: String,
}
