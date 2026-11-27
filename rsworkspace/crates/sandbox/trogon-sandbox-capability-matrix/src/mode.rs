use crate::MatrixError;

/// What the binary was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Fail when the committed document has drifted.
    Check,
    /// Rewrite the generated region of the committed document.
    Write,
}

impl Mode {
    /// Parses the arguments after the program name.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self, MatrixError> {
        let mut mode = Self::Check;
        for arg in args {
            match arg.as_str() {
                "--write" => mode = Self::Write,
                _ => return Err(MatrixError::UnknownArgument(arg)),
            }
        }
        Ok(mode)
    }
}

#[cfg(test)]
mod tests;
