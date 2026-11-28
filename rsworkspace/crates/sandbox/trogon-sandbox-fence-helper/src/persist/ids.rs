use super::*;

/// The SIDs persistent ACEs are written for.
#[derive(Debug, Clone)]
pub struct Ids {
    /// From the install record (administrator-written). `None` when
    /// nothing is installed: grants cannot be made, and `forget` can
    /// no longer tell the group's ACEs apart.
    pub group_sid: Option<String>,
    pub sandbox_user_sid: Option<String>,
    pub real_user_sid: String,
    /// Sandbox groups other than `group_sid` that grants recorded for
    /// this user were made for: the marker's, when the install record
    /// names another group now or none.
    pub previous_group_sids: Vec<String>,
}

impl Ids {
    pub fn load() -> Result<Self> {
        Self::from_records(
            crate::install::read_setup()?,
            read_marker(),
            crate::sid::current_user_sid()?,
        )
    }

    /// An unreadable marker is an error: reading it as empty would drop
    /// the groups the recorded grants were made for, and a revoke or
    /// forget would then leave their entries on disk.
    pub(super) fn from_records(
        setup: Option<crate::install::SetupInfo>,
        marker: Result<Marker>,
        real_user_sid: String,
    ) -> Result<Self> {
        let marker = marker.context("read the persistent-grant marker")?;
        // With the install record gone, the group the recorded grants name
        // is still known from the marker.
        let group_sid = setup
            .as_ref()
            .map(|s| s.sandbox_group_sid.clone())
            .or_else(|| marker.group_sid.clone());
        // Every other group the marker knows of (earlier installs).
        let mut previous_group_sids: Vec<String> = marker
            .group_sid
            .into_iter()
            .chain(marker.previous_group_sids)
            .filter(|g| Some(g) != group_sid.as_ref())
            .collect();
        previous_group_sids.dedup();
        Ok(Self {
            group_sid,
            sandbox_user_sid: setup.map(|s| s.sandbox_user_sid),
            real_user_sid,
            previous_group_sids,
        })
    }

    pub(crate) fn trustees(&self) -> PersistentTrustees<'_> {
        PersistentTrustees {
            group_sid: self.group_sid.as_deref(),
            real_user_sid: &self.real_user_sid,
            previous_group_sids: &self.previous_group_sids,
            sandbox_user_sid: self.sandbox_user_sid.as_deref(),
            recorded_denies: None,
        }
    }

    /// [`Ids::trustees`] for a path with a record: `recorded` are the
    /// denies the record lists for it ([`recorded_denies`]).
    pub(super) fn trustees_for<'a>(&'a self, recorded: Option<&'a [AceView]>) -> PersistentTrustees<'a> {
        PersistentTrustees {
            recorded_denies: recorded,
            ..self.trustees()
        }
    }
}
