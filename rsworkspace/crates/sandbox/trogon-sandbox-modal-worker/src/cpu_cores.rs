use std::str::FromStr;

use serde::Serialize;

use crate::CpuCoresError;

/// A fractional core count for one sandbox.
///
/// It is positive and finite. Modal's `cpu` is a core request, not a CPU-time budget.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(transparent)]
pub struct CpuCores(f64);

impl CpuCores {
    pub fn new(cores: f64) -> Result<Self, CpuCoresError> {
        if cores.is_finite() && cores > 0.0 {
            Ok(Self(cores))
        } else {
            Err(CpuCoresError::NotPositive)
        }
    }

    /// The cores a lease asks for in thousandths of a core.
    pub fn from_millis(millis: u32) -> Result<Self, CpuCoresError> {
        Self::new(f64::from(millis) / 1000.0)
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

impl FromStr for CpuCores {
    type Err = CpuCoresError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value.parse()?)
    }
}
