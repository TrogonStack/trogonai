//! Retires a native snapshot by hand and proves it is gone, with the checks a paid lane needs before it trusts that a
//! snapshot it created no longer exists. The worker has no snapshot delete of its own, so the lane speaks to the
//! snapshot route with its own client.

use std::time::{Duration, Instant};

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;
use url::Url;

use super::{Boxed, TestResult};

const ABSENCE_WAIT: Duration = Duration::from_secs(20);
const ABSENCE_POLL: Duration = Duration::from_millis(250);

#[derive(Deserialize)]
struct Reply {
    snapshot: Metadata,
}

#[derive(Deserialize)]
struct Metadata {
    id: String,
    status: Status,
}

/// The only states Vercel documents for a snapshot. Anything else fails to parse and so never counts as a receipt.
#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Status {
    Created,
    Deleted,
    Failed,
}

pub struct NativeSnapshots {
    http: reqwest::Client,
    endpoint: Url,
    team: String,
    project: String,
}

impl NativeSnapshots {
    pub fn new(endpoint: Url, team: &str, project: &str, token: &str, timeout: Duration) -> Result<Self, Boxed> {
        let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))?;
        authorization.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        Ok(Self {
            http: reqwest::Client::builder()
                .default_headers(headers)
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(timeout)
                .build()?,
            endpoint,
            team: team.to_owned(),
            project: project.to_owned(),
        })
    }

    fn url(&self, id: &str) -> Result<Url, Boxed> {
        let mut url = self.endpoint.clone();
        url.path_segments_mut()
            .map_err(|()| "the native endpoint has no path segments")?
            .pop_if_empty()
            .extend(["v2", "sandboxes", "snapshots", id]);
        url.query_pairs_mut()
            .append_pair("teamId", &self.team)
            .append_pair("projectId", &self.project);
        Ok(url)
    }

    /// The state the answer reports for exactly `id`; a foreign id, a malformed body and a failed capture are all
    /// refused.
    async fn status(response: reqwest::Response, id: &str) -> Result<Status, Boxed> {
        let reply: Reply = response
            .json()
            .await
            .map_err(|_| "the snapshot metadata is not valid")?;
        if reply.snapshot.id != id {
            return Err("the snapshot metadata names another snapshot".into());
        }
        if reply.snapshot.status == Status::Failed {
            return Err("failed snapshot metadata cannot confirm retirement".into());
        }
        Ok(reply.snapshot.status)
    }

    /// Reads the snapshot, deletes it, and waits until it reports deleted or is not found. A delete is only issued
    /// after a positive read of the same identity.
    pub async fn retire(&self, id: &str) -> TestResult {
        let url = self.url(id)?;
        let before = self.http.get(url.clone()).send().await?;
        if !before.status().is_success() {
            return Err("the snapshot must exist before it is retired".into());
        }
        Self::status(before, id).await?;
        let deleted = self.http.delete(url.clone()).send().await?;
        if !deleted.status().is_success() {
            return Err("the snapshot retirement was refused".into());
        }
        Self::status(deleted, id).await?;
        let deadline = Instant::now() + ABSENCE_WAIT;
        loop {
            let response = self.http.get(url.clone()).send().await?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(());
            }
            if !response.status().is_success() {
                return Err("snapshot absence is unknown".into());
            }
            if Self::status(response, id).await? == Status::Deleted {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("the snapshot remains present after retirement".into());
            }
            tokio::time::sleep(ABSENCE_POLL).await;
        }
    }
}
