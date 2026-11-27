//! The toolbox calls that run a command in a session and feed it input, and the file call that sets an owner.

use reqwest::Method;

use crate::daytona_api_sessions_wire::{CreateSessionWire, SessionInputWire};
use crate::safe_id::safe_id;
use crate::session_command::SessionCommand;
use crate::session_command_reply::SessionCommandReply;
use crate::{DaytonaApi, DaytonaApiError};

impl DaytonaApi {
    pub(crate) async fn create_session(&self, id: &str, session_id: &str) -> Result<(), DaytonaApiError> {
        safe_id(session_id, "session id")?;
        let call = self.toolbox(Method::POST, id, "process/session").await?;
        Self::send(call.json(&CreateSessionWire { session_id }))
            .await
            .map(|_| ())
    }

    /// Starts `command` in the session and returns the id the toolbox gave it, when it gave one.
    pub(crate) async fn start_session_command(
        &self,
        id: &str,
        session_id: &str,
        command: &SessionCommand,
    ) -> Result<Option<String>, DaytonaApiError> {
        safe_id(session_id, "session id")?;
        let call = self
            .toolbox(Method::POST, id, &format!("process/session/{session_id}/exec"))
            .await?;
        let bytes = Self::send(call.json(command)).await?;
        Ok(Self::parse::<SessionCommandReply>(&bytes)?.cmd_id)
    }

    /// Sends one line of input to a running session command.
    pub(crate) async fn send_session_input(
        &self,
        id: &str,
        session_id: &str,
        command_id: &str,
        data: &str,
    ) -> Result<(), DaytonaApiError> {
        safe_id(session_id, "session id")?;
        safe_id(command_id, "command id")?;
        let call = self
            .toolbox(
                Method::POST,
                id,
                &format!("process/session/{session_id}/command/{command_id}/input"),
            )
            .await?;
        Self::send(call.json(&SessionInputWire { data })).await.map(|_| ())
    }

    /// Deletes the session; one that is already gone is not an error.
    pub(crate) async fn delete_session(&self, id: &str, session_id: &str) -> Result<(), DaytonaApiError> {
        safe_id(session_id, "session id")?;
        let call = self
            .toolbox(Method::DELETE, id, &format!("process/session/{session_id}"))
            .await?;
        match Self::send(call).await {
            Ok(_) | Err(DaytonaApiError::NotFound(_)) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Sets the mode of `path` and hands it to `owner` and `group`.
    pub(crate) async fn set_permissions_owned(
        &self,
        id: &str,
        path: &str,
        mode: u32,
        owner: &str,
        group: Option<&str>,
    ) -> Result<(), DaytonaApiError> {
        let call = self.toolbox(Method::POST, id, "files/permissions").await?;
        let mut query = vec![
            ("path", path.to_owned()),
            ("mode", format!("{mode:o}")),
            ("owner", owner.to_owned()),
        ];
        if let Some(group) = group {
            query.push(("group", group.to_owned()));
        }
        Self::send(call.query(&query)).await.map(|_| ())
    }
}
