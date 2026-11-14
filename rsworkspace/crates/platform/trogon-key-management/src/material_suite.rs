#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialSuite {
    Dek32V1,
}

impl MaterialSuite {
    pub fn get(self) -> u32 {
        match self {
            Self::Dek32V1 => 1,
        }
    }
}

impl TryFrom<u32> for MaterialSuite {
    type Error = MaterialSuiteError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self::Dek32V1)
        } else {
            Err(MaterialSuiteError)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unsupported key wrapping material suite")]
pub struct MaterialSuiteError;
