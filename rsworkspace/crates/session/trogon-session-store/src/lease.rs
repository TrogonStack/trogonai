use std::{future::Future, time::Instant};

use futures::future::select_all;

use crate::{SessionAdmissionLease, SessionError};

pub(crate) async fn guarded<L, T>(
    leases: &[&L],
    operation_deadline: Instant,
    future: impl Future<Output = Result<T, SessionError>> + Send,
) -> Result<T, SessionError>
where
    L: SessionAdmissionLease,
{
    let deadline = leases
        .iter()
        .fold(operation_deadline, |deadline, lease| deadline.min(lease.deadline()));
    require_live(leases, deadline)?;
    let withdrawn = async {
        if leases.is_empty() {
            std::future::pending::<()>().await;
        } else {
            select_all(leases.iter().map(|lease| Box::pin(lease.withdrawn()))).await;
        }
    };
    let result = tokio::select! {
        biased;
        () = withdrawn => return Err(SessionError::AuthorityWithdrawn),
        () = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => return Err(SessionError::AuthorityExpired),
        result = future => result,
    };
    require_live(leases, deadline)?;
    result
}

pub(crate) fn require_live<L: SessionAdmissionLease>(leases: &[&L], deadline: Instant) -> Result<(), SessionError> {
    if Instant::now() >= deadline || leases.iter().any(|lease| Instant::now() >= lease.deadline()) {
        return Err(SessionError::AuthorityExpired);
    }
    for lease in leases {
        lease
            .require_live()
            .map_err(|error| SessionError::Admission(Box::new(error)))?;
    }
    Ok(())
}
