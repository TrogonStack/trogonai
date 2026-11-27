use reqwest::Method;

use crate::safe_id::safe_id;
use crate::{DaytonaApi, DaytonaApiError, DaytonaSshGrant};

impl DaytonaApi {
    /// Mints a token for Daytona's SSH gateway that opens `id` for `minutes` minutes.
    pub async fn ssh_access(&self, id: &str, minutes: u32) -> Result<DaytonaSshGrant, DaytonaApiError> {
        safe_id(id, "sandbox id")?;
        let request = self
            .control(Method::POST, &format!("/sandbox/{id}/ssh-access"))
            .query(&[("expiresInMinutes", minutes.to_string())]);
        let bytes = Self::send(request).await?;
        Self::parse(&bytes)
    }
}
