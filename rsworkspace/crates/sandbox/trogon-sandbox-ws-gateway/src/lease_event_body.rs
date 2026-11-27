use serde::Serialize;

use crate::lease_row::LeaseRow;

/// A lease change on the dashboard stream.
#[derive(Debug, Serialize)]
pub(crate) struct LeaseEventBody<'a> {
    pub(crate) action: &'static str,
    pub(crate) lease: LeaseRow<'a>,
}
