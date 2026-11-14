//! A scripted host for replaying recorded path-check scenarios.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use serde_json::Value;

use crate::bridge_platform::{EntryKind, Platform, PlatformProbe, ProbeError};

/// Answers every probe call from one scenario's recorded inputs.
pub(crate) struct FakeProbe {
    platform: Platform,
    subsystem: bool,
    kinds: HashMap<String, EntryKind>,
    links: HashMap<String, String>,
    stats: HashMap<String, Result<(EntryKind, u64), ProbeError>>,
    statfs: Mutex<HashMap<String, VecDeque<Result<i64, ProbeError>>>>,
    home: Option<String>,
    master: Option<Result<String, ProbeError>>,
}

fn kind(name: &str) -> EntryKind {
    match name {
        "symlink" => EntryKind::Symlink,
        "file" => EntryKind::File,
        "special" => EntryKind::Special,
        "absent" => EntryKind::Absent,
        "other" => EntryKind::Other,
        _ => EntryKind::Directory,
    }
}

fn failure(value: &Value) -> ProbeError {
    if value["typeError"].as_bool() == Some(true) {
        return ProbeError {
            code: None,
            type_error: true,
        };
    }
    ProbeError::coded(value["code"].as_str().unwrap_or_default())
}

fn map_of<'a>(probe: &'a Value, key: &str) -> impl Iterator<Item = (&'a String, &'a Value)> {
    probe[key].as_object().into_iter().flatten()
}

impl FakeProbe {
    /// The probe a recorded scenario describes.
    pub(crate) fn from_case(case: &Value) -> Self {
        let probe = &case["probe"];
        let platform = match case["platform"].as_str() {
            Some("darwin") => Platform::Darwin,
            Some("win32") => Platform::Win32,
            _ => Platform::Linux,
        };
        let mut subsystem = probe["wsl"].as_bool() == Some(true);
        if let Some(text) = probe["procVersion"].as_str() {
            let text = text.to_lowercase();
            subsystem |= text.contains("microsoft") || text.contains("wsl");
        }
        let stats = map_of(probe, "stats")
            .map(|(path, value)| {
                let answer = if value["code"].is_string() {
                    Err(failure(value))
                } else {
                    Ok((
                        kind(value["kind"].as_str().unwrap_or("dir")),
                        value["dev"].as_u64().unwrap_or(1),
                    ))
                };
                (path.clone(), answer)
            })
            .collect();
        let statfs = map_of(probe, "statfs")
            .map(|(path, list)| {
                let answers = list
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|item| item.as_i64().ok_or_else(|| failure(item)))
                    .collect();
                (path.clone(), answers)
            })
            .collect();
        let master = match &probe["master"] {
            Value::Null => None,
            Value::String(text) => Some(Ok(text.clone())),
            other => Some(Err(failure(other))),
        };
        Self {
            platform,
            subsystem,
            kinds: map_of(probe, "kinds")
                .map(|(path, name)| (path.clone(), kind(name.as_str().unwrap_or_default())))
                .collect(),
            links: map_of(probe, "links")
                .map(|(path, text)| (path.clone(), text.as_str().unwrap_or_default().to_owned()))
                .collect(),
            stats,
            statfs: Mutex::new(statfs),
            home: probe["home"].as_str().map(str::to_owned),
            master,
        }
    }
}

impl PlatformProbe for FakeProbe {
    fn platform(&self) -> Platform {
        self.platform
    }

    fn runtime_platform(&self) -> Platform {
        if self.platform == Platform::Linux && self.subsystem {
            Platform::Wsl
        } else {
            self.platform
        }
    }

    fn entry_kind(&self, path: &str) -> EntryKind {
        self.kinds.get(path).copied().unwrap_or(EntryKind::Directory)
    }

    fn followed_kind(&self, path: &str) -> EntryKind {
        match self.stats.get(path) {
            None => EntryKind::Directory,
            Some(Ok((kind, _))) => *kind,
            Some(Err(error)) => {
                if error.code.as_deref() == Some("ENOENT") {
                    EntryKind::Absent
                } else {
                    EntryKind::Other
                }
            }
        }
    }

    fn link_text(&self, path: &str) -> Option<String> {
        self.links.get(path).filter(|text| !text.contains('\0')).cloned()
    }

    fn statfs_type(&self, path: &str) -> Result<i64, ProbeError> {
        let mut all = self.statfs.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(list) = all.get_mut(path) else {
            return Err(ProbeError::coded("ENOENT"));
        };
        if list.len() > 1 {
            list.pop_front().unwrap_or(Err(ProbeError::default()))
        } else {
            list.front().cloned().unwrap_or(Err(ProbeError::default()))
        }
    }

    fn home_dir(&self) -> Option<String> {
        self.home.clone()
    }

    fn read_text(&self, path: &str) -> Result<String, ProbeError> {
        match (path, &self.master) {
            ("/etc/auto_master", Some(answer)) => answer.clone(),
            _ => Err(ProbeError::coded("ENOENT")),
        }
    }

    fn device_of(&self, path: &str) -> Result<u64, ProbeError> {
        match self.stats.get(path) {
            None => Ok(1),
            Some(Ok((_, device))) => Ok(*device),
            Some(Err(error)) => Err(error.clone()),
        }
    }
}
