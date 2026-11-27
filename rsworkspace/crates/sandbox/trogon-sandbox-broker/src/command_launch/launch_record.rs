use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandLaunch {
    pub fingerprint: String,
    /// When `state` was recorded. Absent on entries written before it was
    /// tracked, which are then never aged out early.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<SystemTime>,
    #[serde(flatten)]
    pub state: LaunchState,
}

impl CommandLaunch {
    pub fn new(fingerprint: impl Into<String>, state: LaunchState) -> Self {
        Self {
            fingerprint: fingerprint.into(),
            recorded_at: Some(SystemTime::now()),
            state,
        }
    }

    /// Whether `state` was recorded more than `age` before `now`.
    pub fn older_than(&self, age: Duration, now: SystemTime) -> bool {
        self.recorded_at
            .and_then(|at| now.duration_since(at).ok())
            .is_some_and(|elapsed| elapsed > age)
    }

    /// The same launch with its stored reply dropped.
    pub fn retired(&self) -> Self {
        Self::new(self.fingerprint.clone(), LaunchState::Retired)
    }
}
