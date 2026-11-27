//! Snapshot calls on the Daytona control plane, and the organization scoping that cleanup of them depends on.

use reqwest::Method;
use serde::{Deserialize, Serialize};

use crate::safe_id::safe_id;
use crate::{DaytonaApi, DaytonaApiError, DaytonaOrganizationId, SandboxRecord, SnapshotRecord};

#[derive(Serialize)]
struct SnapshotRequest<'a> {
    name: &'a str,
}

#[derive(Deserialize)]
struct Identity {
    #[serde(default)]
    id: String,
}

/// A snapshot name or id as it travels in a URL path: the id alphabet plus dots, and never a path step.
fn path_segment(value: &str) -> Result<(), DaytonaApiError> {
    if value == "." || value == ".." {
        return Err(DaytonaApiError::Rejected {
            status: 400,
            message: "invalid snapshot name".to_owned(),
        });
    }
    safe_id(&value.replace('.', "-"), "snapshot name")
}

impl DaytonaApi {
    /// A client that sends every request for `organization`. A key that is already tied to another organization is
    /// refused, so retained ownership never moves to a different account.
    pub fn for_organization(&self, organization: &DaytonaOrganizationId) -> Result<Self, DaytonaApiError> {
        if self
            .config()
            .organization()
            .is_some_and(|configured| configured != organization)
        {
            return Err(DaytonaApiError::Rejected {
                status: 400,
                message: "the configured organization conflicts with the retained owner".to_owned(),
            });
        }
        Self::new(
            self.config().clone().with_organization(organization.clone()),
            self.timeout(),
        )
    }

    /// Confirms the current credentials can read exactly `organization`.
    pub async fn verify_organization(&self, organization: &DaytonaOrganizationId) -> Result<(), DaytonaApiError> {
        safe_id(organization.as_str(), "organization id")?;
        let bytes = Self::send(self.control(Method::GET, &format!("/organizations/{}", organization.as_str()))).await?;
        let identity: Identity = Self::parse(&bytes)?;
        if identity.id == organization.as_str() {
            Ok(())
        } else {
            Err(DaytonaApiError::Transport(
                "the organization identity does not match the retained owner".to_owned(),
            ))
        }
    }

    /// The sandbox, or `None` when Daytona answers 404. Any other failure stays an error.
    pub async fn find_sandbox(&self, id: &str) -> Result<Option<SandboxRecord>, DaytonaApiError> {
        match self.get_sandbox(id).await {
            Ok(record) => Ok(Some(record)),
            Err(DaytonaApiError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The snapshot with this name or id, or `None` when Daytona answers 404. Any other failure stays an error.
    pub async fn find_snapshot(&self, name_or_id: &str) -> Result<Option<SnapshotRecord>, DaytonaApiError> {
        path_segment(name_or_id)?;
        match Self::send(self.control(Method::GET, &format!("/snapshots/{name_or_id}"))).await {
            Ok(bytes) => Self::parse(&bytes).map(Some),
            Err(DaytonaApiError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Asks Daytona to snapshot a sandbox under `name`. Daytona answers before the snapshot is built, so the answer
    /// names the source sandbox and says nothing about the snapshot.
    pub async fn snapshot_sandbox(&self, sandbox_id: &str, name: &str) -> Result<SandboxRecord, DaytonaApiError> {
        safe_id(sandbox_id, "sandbox id")?;
        path_segment(name)?;
        let request = self
            .control(Method::POST, &format!("/sandbox/{sandbox_id}/snapshot"))
            .json(&SnapshotRequest { name });
        let bytes = Self::send(request).await?;
        Self::parse(&bytes)
    }

    /// Deletes the snapshot. A snapshot that is already gone is deleted.
    pub async fn delete_snapshot(&self, id: &str) -> Result<(), DaytonaApiError> {
        path_segment(id)?;
        match Self::send(self.control(Method::DELETE, &format!("/snapshots/{id}"))).await {
            Ok(_) | Err(DaytonaApiError::NotFound(_)) => Ok(()),
            Err(error) => Err(error),
        }
    }
}
