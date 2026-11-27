use std::fmt::Display;
use std::io::Write;

use crate::AdminError;

/// Where a command writes: results go to `out`, notices for the operator to `err`.
pub struct Console<'a> {
    out: &'a mut (dyn Write + Send),
    err: &'a mut (dyn Write + Send),
}

impl<'a> Console<'a> {
    pub fn new(out: &'a mut (dyn Write + Send), err: &'a mut (dyn Write + Send)) -> Self {
        Self { out, err }
    }

    pub fn out(&mut self, line: impl Display) -> Result<(), AdminError> {
        writeln!(self.out, "{line}").map_err(AdminError::Output)
    }

    pub fn err(&mut self, line: impl Display) -> Result<(), AdminError> {
        writeln!(self.err, "{line}").map_err(AdminError::Output)
    }

    pub fn out_json<T: serde::Serialize>(&mut self, value: &T) -> Result<(), AdminError> {
        let text = serde_json::to_string_pretty(value).map_err(AdminError::Encode)?;
        self.out(text)
    }
}
